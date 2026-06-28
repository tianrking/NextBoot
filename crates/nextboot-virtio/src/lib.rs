//! 虚拟 Block IO 驱动
//!
//! 这是 NextBoot 的核心模块，实现 ISO 文件到虚拟设备的映射。
//!
//! # 工作原理
//! 1. 将 ISO 文件在物理设备上的位置映射为虚拟 LBA
//! 2. 拦截读取请求，转换为物理设备读取
//! 3. 拦截写入请求，返回只读错误
//!
//! # 需求对应
//! - 模块 B: 虚拟化层 (P0)

#![no_std]

extern crate alloc;

mod block_io;
mod manager;
mod model;

pub mod mapping;
pub mod protocol;

pub use block_io::{MemoryOverlay, PhysicalReadFn, PhysicalReader, VirtualBlockIo};
pub use manager::VirtualDeviceManager;
pub use model::{
    CdRomBootInfo, IsoMapping, MediaFlags, VirtIoError, VirtualDeviceConfig, VirtualDeviceInfo,
    VirtualDeviceType, VirtualMediaInfo,
};

#[cfg(test)]
mod tests;
