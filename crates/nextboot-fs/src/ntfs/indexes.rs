use super::parser::parse_index_entries;
use super::*;

impl Ntfs {
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
}
