//! FAT32 文件系统实现
//!
//! 仅支持读取，用于 ESP 分区和 Data 分区

#[path = "fat32/directory.rs"]
mod directory;
#[path = "fat32/extent.rs"]
mod extent;
#[path = "fat32/fs.rs"]
mod fs;
#[path = "fat32/model.rs"]
mod model;

pub use fs::{is_fat32, Fat32, Fat32Info};
