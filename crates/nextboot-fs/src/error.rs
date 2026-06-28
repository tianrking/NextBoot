/// 文件系统错误类型
#[derive(Debug, Clone, Copy)]
pub enum FsError {
    /// 无效的文件系统签名
    InvalidSignature,
    /// 块大小不匹配
    BlockSizeMismatch,
    /// 文件未找到
    FileNotFound,
    /// 读取错误
    ReadError,
    /// 内存不足
    OutOfMemory,
    /// 无效路径
    InvalidPath,
    /// 不支持的文件系统
    UnsupportedFs,
    /// 无效参数
    InvalidArgument,
    /// 目录不存在
    DirectoryNotFound,
    /// 不是目录
    NotDirectory,
    /// 不是文件
    NotFile,
    /// 文件太大
    FileTooLarge,
    /// 损坏的文件系统
    Corrupted,
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FsError::InvalidSignature => write!(f, "Invalid filesystem signature"),
            FsError::BlockSizeMismatch => write!(f, "Block size mismatch"),
            FsError::FileNotFound => write!(f, "File not found"),
            FsError::ReadError => write!(f, "Read error"),
            FsError::OutOfMemory => write!(f, "Out of memory"),
            FsError::InvalidPath => write!(f, "Invalid path"),
            FsError::UnsupportedFs => write!(f, "Unsupported filesystem"),
            FsError::InvalidArgument => write!(f, "Invalid argument"),
            FsError::DirectoryNotFound => write!(f, "Directory not found"),
            FsError::NotDirectory => write!(f, "Not a directory"),
            FsError::NotFile => write!(f, "Not a file"),
            FsError::FileTooLarge => write!(f, "File too large"),
            FsError::Corrupted => write!(f, "Corrupted filesystem"),
        }
    }
}
