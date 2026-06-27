//! exFAT 文件系统实现
//!
//! 用于 Data 分区，支持 >4GB 文件

#[path = "exfat/directory.rs"]
mod directory;
#[path = "exfat/extent.rs"]
mod extent;
#[path = "exfat/fs.rs"]
mod fs;
#[path = "exfat/model.rs"]
mod model;

pub use fs::{is_exfat, ExFat, ExFatInfo};
