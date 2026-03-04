//! UEFI 初始化模块
//!
//! 负责 UEFI 服务初始化和设备检测

use uefi::prelude::*;
use uefi::table::boot::{BootServices, BlockIO};
use uefi::proto::device_path::text::{DevicePathToText, DisplayOnly};
use alloc::vec::Vec;
use alloc::string::String;

/// 初始化 UEFI 服务
pub fn uefi_services(image: &Handle, st: &SystemTable<Boot>) -> uefi::Result<()> {
    // 初始化 uefi-services crate
    uefi_services::init(image, st).map_err(|e| uefi::Status::from(e))?;

    // 初始化日志
    log::set_max_level(log::LevelFilter::Info);

    Ok(())
}

/// 存储设备信息
#[derive(Debug, Clone)]
pub struct StorageDevice {
    /// 设备路径
    pub path: String,
    /// 是否为可移动设备
    pub removable: bool,
    /// 块大小 (512 或 4096)
    pub block_size: u32,
    /// 总块数
    pub total_blocks: u64,
    /// Block IO 协议句柄
    pub block_io: Handle,
}

/// 检测所有存储设备
pub fn detect_storage_devices(bt: &BootServices) -> uefi::Result<Vec<StorageDevice>> {
    let mut devices = Vec::new();

    // 获取所有支持 BlockIO 的设备句柄
    let handles = bt.find_handles::<BlockIO>()?;

    for handle in handles {
        // 打开 BlockIO 协议
        let block_io = bt.open_protocol::<BlockIO>(
            handle,
            uefi::table::boot::OpenProtocolAttributes::Exclusive,
        )?;

        let media = block_io.media();

        // 只处理物理设备 (非逻辑分区)
        if !media.has_media() {
            continue;
        }

        // 获取设备路径文本
        let device_path = bt.open_protocol::<DevicePathToText>(
            handle,
            uefi::table::boot::OpenProtocolAttributes::Exclusive,
        );

        let path_str = if let Ok(dp) = device_path {
            // 使用 DevicePathToText 转换
            // 注意: 简化实现，实际需要更复杂的处理
            alloc::format!("Device({})", handle.as_ptr() as usize)
        } else {
            alloc::format!("Unknown-{}", handle.as_ptr() as usize)
        };

        devices.push(StorageDevice {
            path: path_str,
            removable: media.is_removable(),
            block_size: media.block_size(),
            total_blocks: media.last_block() + 1,
            block_io: handle,
        });

        // 关闭协议
        bt.close_protocol(handle);
    }

    Ok(devices)
}

/// 检查是否为 4K Native 设备
pub fn is_4k_native(device: &StorageDevice) -> bool {
    device.block_size == 4096
}
