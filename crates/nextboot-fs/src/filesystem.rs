use crate::{FileExtent, FileInfo, FileSystemType, FsError, SharedBlockIo};
use alloc::vec::Vec;

/// 文件系统 trait - 所有文件系统必须实现
pub trait FileSystem: Sized {
    /// 文件系统类型
    const FS_TYPE: FileSystemType;

    /// 从 Block IO 初始化文件系统
    fn init(block_io: SharedBlockIo) -> Result<Self, FsError>;

    /// 读取目录内容
    fn read_dir(&self, path: &str) -> Result<Vec<FileInfo>, FsError>;

    /// 读取文件内容到缓冲区
    fn read_file(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// 获取文件信息
    fn stat(&self, path: &str) -> Result<FileInfo, FsError>;

    /// 获取块大小
    fn block_size(&self) -> u32;

    /// 获取文件到底层块设备的物理 LBA 映射。
    fn file_extents(&self, _path: &str) -> Result<Vec<FileExtent>, FsError> {
        Err(FsError::UnsupportedFs)
    }

    /// 递归扫描目录获取所有文件
    fn scan_files(&self, path: &str, extensions: &[&str]) -> Result<Vec<FileInfo>, FsError> {
        let mut result = Vec::new();
        self.scan_files_recursive(path, extensions, &mut result)?;
        Ok(result)
    }

    /// 递归扫描辅助函数
    fn scan_files_recursive(
        &self,
        path: &str,
        extensions: &[&str],
        result: &mut Vec<FileInfo>,
    ) -> Result<(), FsError> {
        let entries = self.read_dir(path)?;

        for entry in entries {
            // 跳过隐藏和系统文件
            if entry.is_hidden() || entry.is_system() {
                continue;
            }

            let full_path = if path == "/" || path.is_empty() {
                alloc::format!("/{}", entry.name)
            } else {
                alloc::format!("{}/{}", path, entry.name)
            };

            if entry.is_dir {
                // 递归扫描子目录
                self.scan_files_recursive(&full_path, extensions, result)?;
            } else {
                // 检查扩展名
                let name_lower = entry.name.to_ascii_lowercase();
                let matches =
                    extensions.is_empty() || extensions.iter().any(|ext| name_lower.ends_with(ext));

                if matches {
                    let mut file_info = entry.clone();
                    file_info.name = full_path;
                    result.push(file_info);
                }
            }
        }

        Ok(())
    }
}
