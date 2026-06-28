//! NextBoot 文件系统模块
//!
//! 提供 FAT32、exFAT 和 ISO9660 文件系统的只读支持。
//!
//! # 设计原则
//! - 所有操作都是只读的 (符合 PRD 要求)
//! - 支持动态块大小检测 (4K/512B)
//! - 零拷贝设计，最小化内存使用

#![no_std]

extern crate alloc;

mod block_io;
mod buffer;
mod detect;
mod error;
mod filesystem;
mod paths;
mod types;

pub mod exfat;
pub mod ext4;
pub mod fat32;
pub mod gpt;
pub mod iso9660;
pub mod ntfs;
pub mod udf;
pub mod xfs;

pub use block_io::{read_full_blocks, BlockIoOps, DynBlockIo, SharedBlockIo};
pub use buffer::alloc_buffer;
pub use detect::{detect_fs_type, detect_iso_type};
pub use error::FsError;
pub use filesystem::FileSystem;
pub use paths::{normalize_path, split_path};
pub use types::{FileAttributes, FileExtent, FileInfo, FileSystemType};

#[cfg(test)]
extern crate std;

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
