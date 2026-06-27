use super::parser::{parse_attribute, parse_attribute_list_entries, parse_index_entries};
use super::*;

impl Ntfs {
    pub(super) fn path_to_record(&self, path: &str) -> Result<u64, FsError> {
        let mut record = MFT_RECORD_ROOT;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let entries = self.read_directory(record)?;
            let entry = entries
                .into_iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(part))
                .ok_or(FsError::DirectoryNotFound)?;
            if !entry.is_dir {
                return Err(FsError::NotDirectory);
            }
            record = entry.start_cluster;
        }
        Ok(record)
    }

    pub(super) fn read_directory(&self, record_number: u64) -> Result<Vec<FileInfo>, FsError> {
        let record = self.read_file_record(record_number)?;
        if !record.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let mut entries = Vec::new();
        if let Some(index_root) = record.attribute(ATTR_TYPE_INDEX_ROOT) {
            self.parse_index_root(index_root, &mut entries)?;
        }
        let index_allocations = record.attributes(ATTR_TYPE_INDEX_ALLOCATION)?;
        if !index_allocations.is_empty() {
            self.parse_index_allocations(&index_allocations, &mut entries)?;
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
        Ok(entries)
    }

    pub(super) fn read_boot_mft_record(&self) -> Result<Vec<u8>, FsError> {
        let mut data = alloc_buffer(self.file_record_size as usize)?;
        let byte_offset = self
            .mft_lcn
            .checked_mul(self.cluster_size)
            .ok_or(FsError::Corrupted)?;
        self.read_physical_bytes(byte_offset, &mut data)?;
        Ok(data)
    }

    pub(super) fn read_file_record(&self, record_number: u64) -> Result<FileRecord, FsError> {
        let mut record = self.read_file_record_base(record_number)?;
        self.expand_attribute_list(&mut record)?;
        Ok(record)
    }

    pub(super) fn read_file_record_base(&self, record_number: u64) -> Result<FileRecord, FsError> {
        let mut data = alloc_buffer(self.file_record_size as usize)?;
        let offset = record_number
            .checked_mul(u64::from(self.file_record_size))
            .ok_or(FsError::Corrupted)?;
        self.read_from_runs(&self.mft_runs, offset, &mut data, false)?;
        self.parse_file_record(record_number, data)
    }

    pub(super) fn parse_file_record(
        &self,
        record_number: u64,
        mut data: Vec<u8>,
    ) -> Result<FileRecord, FsError> {
        if data.len() < 0x30 || &data[0..4] != FILE_RECORD_MAGIC {
            return Err(FsError::Corrupted);
        }

        apply_update_sequence(&mut data, self.bytes_per_sector as usize)?;

        let attrs_offset = read_u16(&data, 0x14)? as usize;
        let flags = read_u16(&data, 0x16)?;
        if flags & FILE_FLAG_IN_USE == 0 {
            return Err(FsError::FileNotFound);
        }
        if attrs_offset >= data.len() {
            return Err(FsError::Corrupted);
        }

        let mut attributes = Vec::new();
        let mut offset = attrs_offset;
        while offset + 8 <= data.len() {
            let attr_type = read_u32(&data, offset)?;
            if attr_type == ATTR_TYPE_END {
                break;
            }

            let attr_len = read_u32(&data, offset + 4)? as usize;
            if attr_len == 0
                || offset
                    .checked_add(attr_len)
                    .map_or(true, |end| end > data.len())
            {
                return Err(FsError::Corrupted);
            }

            let attr_data = &data[offset..offset + attr_len];
            if let Some(attribute) = parse_attribute(attr_type, attr_data)? {
                attributes
                    .try_reserve_exact(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                attributes.push(attribute);
            }
            offset += attr_len;
        }

        Ok(FileRecord {
            record_number,
            flags,
            attributes,
        })
    }

    pub(super) fn expand_attribute_list(&self, record: &mut FileRecord) -> Result<(), FsError> {
        let entries = self.attribute_list_entries(record)?;
        if entries.is_empty() {
            return Ok(());
        }

        let mut extension_records = Vec::new();
        for entry in &entries {
            if entry.record_number == record.record_number
                || entry.attr_type == ATTR_TYPE_ATTRIBUTE_LIST
                || extension_records.contains(&entry.record_number)
            {
                continue;
            }
            extension_records
                .try_reserve_exact(1)
                .map_err(|_| FsError::OutOfMemory)?;
            extension_records.push(entry.record_number);
        }

        for record_number in extension_records {
            let extension = self.read_file_record_base(record_number)?;
            for attribute in extension.attributes {
                if attribute.attr_type == ATTR_TYPE_ATTRIBUTE_LIST {
                    continue;
                }
                if !entries.iter().any(|entry| {
                    entry.record_number == record_number && entry.attr_type == attribute.attr_type
                }) {
                    continue;
                }
                record
                    .attributes
                    .try_reserve_exact(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                record.attributes.push(attribute);
            }
        }

        record.attributes.sort_by(|a, b| {
            a.attr_type
                .cmp(&b.attr_type)
                .then_with(|| a.name_len.cmp(&b.name_len))
                .then_with(|| a.lowest_vcn.cmp(&b.lowest_vcn))
        });
        Ok(())
    }

    pub(super) fn attribute_list_entries(
        &self,
        record: &FileRecord,
    ) -> Result<Vec<AttributeListEntry>, FsError> {
        let mut entries = Vec::new();
        for attribute in record.attributes(ATTR_TYPE_ATTRIBUTE_LIST)? {
            let size = attribute.data_size()?;
            if size == 0 {
                continue;
            }
            let size = usize::try_from(size).map_err(|_| FsError::FileTooLarge)?;
            let mut data = alloc_buffer(size)?;
            self.read_attribute(attribute, 0, size as u64, &mut data)?;
            parse_attribute_list_entries(&data, &mut entries)?;
        }
        Ok(entries)
    }

    pub(super) fn parse_index_root(
        &self,
        attribute: &NtfsAttribute,
        entries: &mut Vec<FileInfo>,
    ) -> Result<(), FsError> {
        let value = attribute.resident_value()?;
        if value.len() < 32 {
            return Err(FsError::Corrupted);
        }

        let index_header = 16usize;
        let entries_offset = read_u32(value, index_header)? as usize;
        let total_size = read_u32(value, index_header + 4)? as usize;
        let flags = *value.get(index_header + 12).ok_or(FsError::Corrupted)?;
        let start = index_header
            .checked_add(entries_offset)
            .ok_or(FsError::Corrupted)?;
        let end = index_header
            .checked_add(total_size)
            .ok_or(FsError::Corrupted)?
            .min(value.len());
        if start > end {
            return Err(FsError::Corrupted);
        }

        parse_index_entries(&value[start..end], entries)?;
        if flags & INDEX_HEADER_HAS_ALLOCATION != 0 {
            // The caller will parse $INDEX_ALLOCATION when it is present.
            return Ok(());
        }
        Ok(())
    }

    pub(super) fn parse_index_allocations(
        &self,
        attributes: &[&NtfsAttribute],
        entries: &mut Vec<FileInfo>,
    ) -> Result<(), FsError> {
        let runs = collect_nonresident_runs(attributes)?;
        let size = runs
            .iter()
            .filter_map(|run| {
                run.virtual_cluster_start
                    .checked_add(run.cluster_count)
                    .and_then(|cluster| cluster.checked_mul(self.cluster_size))
            })
            .max()
            .unwrap_or(0);
        if size == 0 {
            return Ok(());
        }

        let size = usize::try_from(size).map_err(|_| FsError::FileTooLarge)?;
        let mut data = alloc_buffer(size)?;
        self.read_from_runs(&runs, 0, &mut data, true)?;

        let record_size = self.index_record_size as usize;
        if record_size == 0 {
            return Err(FsError::Corrupted);
        }

        for chunk in data.chunks_mut(record_size) {
            if chunk.len() < record_size || &chunk[0..4] != INDEX_RECORD_MAGIC {
                continue;
            }
            apply_update_sequence(chunk, self.bytes_per_sector as usize)?;
            if chunk.len() < 40 {
                return Err(FsError::Corrupted);
            }

            let index_header = 24usize;
            let entries_offset = read_u32(chunk, index_header)? as usize;
            let total_size = read_u32(chunk, index_header + 4)? as usize;
            let start = index_header
                .checked_add(entries_offset)
                .ok_or(FsError::Corrupted)?;
            let end = index_header
                .checked_add(total_size)
                .ok_or(FsError::Corrupted)?
                .min(chunk.len());
            if start > end {
                return Err(FsError::Corrupted);
            }
            parse_index_entries(&chunk[start..end], entries)?;
        }

        Ok(())
    }

    pub(super) fn read_attribute(
        &self,
        attribute: &NtfsAttribute,
        offset: u64,
        file_size: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= file_size || buf.is_empty() {
            return Ok(0);
        }

        let to_read = buf
            .len()
            .min(usize::try_from(file_size - offset).map_err(|_| FsError::FileTooLarge)?);
        match &attribute.data {
            AttributeData::Resident { value } => {
                let start = usize::try_from(offset).map_err(|_| FsError::FileTooLarge)?;
                let end = start.checked_add(to_read).ok_or(FsError::Corrupted)?;
                let source = value.get(start..end).ok_or(FsError::ReadError)?;
                buf[..to_read].copy_from_slice(source);
                Ok(to_read)
            }
            AttributeData::NonResident { runs, .. } => {
                self.read_from_runs(runs, offset, &mut buf[..to_read], true)?;
                Ok(to_read)
            }
        }
    }

    pub(super) fn read_attributes(
        &self,
        attributes: &[&NtfsAttribute],
        offset: u64,
        file_size: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if attributes.is_empty() {
            return Err(FsError::NotFile);
        }
        if attributes.len() == 1 {
            return self.read_attribute(attributes[0], offset, file_size, buf);
        }
        if attributes
            .iter()
            .any(|attribute| matches!(&attribute.data, AttributeData::Resident { .. }))
        {
            return Err(FsError::UnsupportedFs);
        }
        if offset >= file_size || buf.is_empty() {
            return Ok(0);
        }

        let to_read = buf
            .len()
            .min(usize::try_from(file_size - offset).map_err(|_| FsError::FileTooLarge)?);
        let runs = collect_nonresident_runs(attributes)?;
        self.read_from_runs(&runs, offset, &mut buf[..to_read], true)?;
        Ok(to_read)
    }

    pub(super) fn read_from_runs(
        &self,
        runs: &[DataRun],
        offset: u64,
        buf: &mut [u8],
        zero_sparse: bool,
    ) -> Result<(), FsError> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(FsError::Corrupted)?;
        let mut cursor = offset;
        let mut copied = 0usize;

        while copied < buf.len() {
            let run = runs
                .iter()
                .find(|run| {
                    let start = run.virtual_cluster_start.saturating_mul(self.cluster_size);
                    let end =
                        start.saturating_add(run.cluster_count.saturating_mul(self.cluster_size));
                    cursor >= start && cursor < end
                })
                .ok_or(FsError::ReadError)?;
            let run_start = run
                .virtual_cluster_start
                .checked_mul(self.cluster_size)
                .ok_or(FsError::Corrupted)?;
            let run_bytes = run
                .cluster_count
                .checked_mul(self.cluster_size)
                .ok_or(FsError::Corrupted)?;
            let run_end = run_start.checked_add(run_bytes).ok_or(FsError::Corrupted)?;
            let read_end = end.min(run_end);
            let read_len = usize::try_from(read_end - cursor).map_err(|_| FsError::FileTooLarge)?;

            if let Some(logical_cluster_start) = run.logical_cluster_start {
                let physical_byte = logical_cluster_start
                    .checked_mul(self.cluster_size)
                    .and_then(|start| start.checked_add(cursor - run_start))
                    .ok_or(FsError::Corrupted)?;
                self.read_physical_bytes(physical_byte, &mut buf[copied..copied + read_len])?;
            } else if zero_sparse {
                buf[copied..copied + read_len].fill(0);
            } else {
                return Err(FsError::UnsupportedFs);
            }

            cursor = read_end;
            copied += read_len;
        }

        Ok(())
    }

    pub(super) fn read_physical_bytes(
        &self,
        physical_byte: u64,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        if buf.is_empty() {
            return Ok(());
        }

        let block_size = self.bytes_per_sector as u64;
        if block_size == 0 {
            return Err(FsError::InvalidArgument);
        }
        let end = physical_byte
            .checked_add(buf.len() as u64)
            .ok_or(FsError::ReadError)?;
        let disk_size = self
            .total_sectors
            .checked_mul(block_size)
            .ok_or(FsError::ReadError)?;
        if end > disk_size {
            return Err(FsError::ReadError);
        }

        let mut copied = 0usize;
        let mut cursor = physical_byte;
        let mut block = alloc_buffer(self.bytes_per_sector as usize)?;
        while copied < buf.len() {
            let lba = cursor / block_size;
            let in_block = (cursor % block_size) as usize;
            read_full_blocks(self.block_io.as_ref(), lba, &mut block)?;
            let available = block.len().saturating_sub(in_block);
            let to_copy = available.min(buf.len() - copied);
            buf[copied..copied + to_copy].copy_from_slice(&block[in_block..in_block + to_copy]);
            copied += to_copy;
            cursor = cursor
                .checked_add(to_copy as u64)
                .ok_or(FsError::ReadError)?;
        }

        Ok(())
    }

    pub(super) fn runs_to_extents(
        &self,
        runs: &[DataRun],
        file_size: u64,
    ) -> Result<Vec<FileExtent>, FsError> {
        let mut extents = Vec::new();
        if file_size == 0 {
            return Ok(extents);
        }

        let blocks_per_cluster = u64::from(self.sectors_per_cluster);
        let mut remaining_blocks =
            div_round_up(file_size, u64::from(self.bytes_per_sector)).ok_or(FsError::Corrupted)?;

        for run in runs {
            let Some(logical_cluster_start) = run.logical_cluster_start else {
                return Err(FsError::UnsupportedFs);
            };
            let run_blocks = run
                .cluster_count
                .checked_mul(blocks_per_cluster)
                .ok_or(FsError::Corrupted)?;
            let block_count = run_blocks.min(remaining_blocks);
            if block_count == 0 {
                break;
            }

            push_extent(
                &mut extents,
                run.virtual_cluster_start
                    .checked_mul(blocks_per_cluster)
                    .ok_or(FsError::Corrupted)?,
                logical_cluster_start
                    .checked_mul(blocks_per_cluster)
                    .ok_or(FsError::Corrupted)?,
                block_count,
            );
            remaining_blocks -= block_count;
            if remaining_blocks == 0 {
                break;
            }
        }

        if remaining_blocks == 0 {
            Ok(extents)
        } else {
            Err(FsError::Corrupted)
        }
    }
}
