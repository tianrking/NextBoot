use super::*;

impl Ext4 {
    pub(super) fn read_dir_node(&self, node: &Ext4Node) -> Result<Vec<FileInfo>, FsError> {
        let entries = self.read_dir_entries(node)?;
        let mut out = Vec::new();
        out.try_reserve_exact(entries.len())
            .map_err(|_| FsError::OutOfMemory)?;
        for entry in entries {
            let child = self.read_inode(entry.inode_number)?;
            out.push(self.info_for_node(entry.name, &child));
        }
        Ok(out)
    }

    pub(super) fn read_dir_entries(&self, node: &Ext4Node) -> Result<Vec<Ext4DirEntry>, FsError> {
        let mut data =
            alloc_buffer(usize::try_from(node.size).map_err(|_| FsError::FileTooLarge)?)?;
        if !data.is_empty() {
            self.read_node_data(node, 0, &mut data)?;
        }

        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset + 8 <= data.len() {
            let inode_number = read_u32(&data, offset)?;
            let rec_len = read_u16(&data, offset + 4)? as usize;
            let name_len = data[offset + 6] as usize;
            if rec_len < 8 || offset + rec_len > data.len() || name_len > rec_len - 8 {
                return Err(FsError::Corrupted);
            }
            if inode_number != 0 {
                let name = String::from_utf8(data[offset + 8..offset + 8 + name_len].to_vec())
                    .map_err(|_| FsError::Corrupted)?;
                if name != "." && name != ".." {
                    entries.push(Ext4DirEntry { inode_number, name });
                }
            }
            offset += rec_len;
        }
        Ok(entries)
    }
}
