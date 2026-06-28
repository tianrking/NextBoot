//! UEFI 初始化模块
//!
//! 负责 UEFI 服务初始化和设备检测

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use log::debug;
use uefi::prelude::*;
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::{BootServices, SearchType};
use uefi::Identify;

/// 初始化 UEFI 服务
pub fn uefi_services(st: &mut SystemTable<Boot>) -> uefi::Result<()> {
    // 初始化 uefi-services crate
    uefi_services::init(st).map(|_| ())?;

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
    /// 总大小 (字节)
    pub total_size: u64,
    /// Block IO 协议句柄
    pub block_io: Handle,
    /// 设备索引
    pub index: usize,
}

impl StorageDevice {
    /// 获取设备描述
    pub fn description(&self) -> String {
        let device_type = if self.removable { "Removable" } else { "Fixed" };
        let sector_type = if self.block_size == 4096 {
            "4K"
        } else {
            "512B"
        };

        format!(
            "{} {} ({}, {} sectors)",
            device_type,
            format_size(self.total_size),
            sector_type,
            self.total_blocks
        )
    }
}

/// 检测所有存储设备
pub fn detect_storage_devices(bt: &BootServices) -> uefi::Result<Vec<StorageDevice>> {
    let mut devices = Vec::new();

    // 获取所有支持 BlockIO 的设备句柄
    let handles = bt.locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))?;

    for (index, handle) in handles.iter().copied().enumerate() {
        // 打开 BlockIO 协议
        let block_io = match bt.open_protocol_exclusive::<BlockIO>(handle) {
            Ok(protocol) => protocol,
            Err(_) => continue,
        };

        let media = block_io.media();

        // 只处理有媒体的设备
        if !media.is_media_present() {
            debug!("Skipping device without media");
            continue;
        }

        // 跳过逻辑分区 (只处理物理设备)
        if media.is_logical_partition() {
            debug!("Skipping logical partition");
            continue;
        }

        let block_size = media.block_size();
        let total_blocks = media.last_block() + 1;
        let total_size = total_blocks * block_size as u64;

        // 生成设备路径描述
        let path = format!("Device{}", index);

        devices.push(StorageDevice {
            path,
            removable: media.is_removable_media(),
            block_size,
            total_blocks,
            total_size,
            block_io: handle,
            index,
        });
    }

    // 排序: 可移动设备优先
    devices.sort_by(|a, b| {
        b.removable
            .cmp(&a.removable)
            .then_with(|| a.index.cmp(&b.index))
    });

    Ok(devices)
}

/// 检查是否为 4K Native 设备
pub fn is_4k_native(device: &StorageDevice) -> bool {
    device.block_size == 4096
}

/// 查找 ESP 分区
pub fn find_esp_partition(bt: &BootServices) -> uefi::Result<Option<Handle>> {
    let handles = bt.locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))?;

    for handle in handles.iter().copied() {
        let block_io = match bt.open_protocol_exclusive::<BlockIO>(handle) {
            Ok(protocol) => protocol,
            Err(_) => continue,
        };

        let media = block_io.media();

        if !media.is_media_present() {
            continue;
        }

        // 检查是否为 FAT32 (简化检测)
        // ESP 通常是 FAT32 格式，大小较小

        let block_size = media.block_size();
        let total_blocks = media.last_block() + 1;
        let total_size = total_blocks * block_size as u64;

        // ESP 通常小于 1GB
        if total_size < 1024 * 1024 * 1024 {
            // TODO: 更精确的检测方法
            return Ok(Some(handle));
        }
    }

    Ok(None)
}

/// 获取启动设备
pub fn get_boot_device(bt: &BootServices, image: Handle) -> uefi::Result<Option<Handle>> {
    // 获取加载的镜像协议
    use uefi::proto::loaded_image::LoadedImage;

    let loaded_image = bt.open_protocol_exclusive::<LoadedImage>(image)?;

    // 获取设备句柄
    let device_handle = loaded_image.device();

    Ok(device_handle)
}

/// 格式化大小
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// 内存信息
#[derive(Debug, Clone)]
pub struct MemoryInfo {
    /// 总内存 (字节)
    pub total: u64,
    /// 可用内存 (字节)
    pub available: u64,
    /// 使用率
    pub used_percent: f32,
}

/// 获取内存信息
pub fn get_memory_info(bt: &BootServices) -> uefi::Result<MemoryInfo> {
    use core::mem;
    use core::slice;
    use uefi::table::boot::{MemoryDescriptor, MemoryType};

    let map_size = bt.memory_map_size();
    let extra_entries = 8;
    let buffer_size = map_size.map_size + map_size.entry_size * extra_entries;
    let descriptor_count =
        (buffer_size + mem::size_of::<MemoryDescriptor>() - 1) / mem::size_of::<MemoryDescriptor>();
    let mut backing = alloc::vec![MemoryDescriptor::default(); descriptor_count];
    let buffer = unsafe {
        slice::from_raw_parts_mut(
            backing.as_mut_ptr().cast::<u8>(),
            backing.len() * mem::size_of::<MemoryDescriptor>(),
        )
    };

    let memory_map = bt.memory_map(buffer)?;
    let mut total = 0u64;
    let mut available = 0u64;

    for descriptor in memory_map.entries() {
        let size = descriptor.page_count * 4096;
        total += size;

        if descriptor.ty == MemoryType::CONVENTIONAL {
            available += size;
        }
    }

    let used_percent = if total > 0 {
        ((total - available) as f64 / total as f64 * 100.0) as f32
    } else {
        0.0
    };

    Ok(MemoryInfo {
        total,
        available,
        used_percent,
    })
}

/// 系统信息
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// UEFI 版本
    pub uefi_version: (u16, u16),
    /// 固件厂商
    pub firmware_vendor: String,
    /// 固件版本
    pub firmware_revision: u32,
    /// 内存信息
    pub memory: MemoryInfo,
    /// 存储设备数量
    pub storage_device_count: usize,
}

/// 获取系统信息
pub fn get_system_info(bt: &BootServices, st: &SystemTable<Boot>) -> uefi::Result<SystemInfo> {
    let revision = st.uefi_revision();
    let firmware_vendor = String::from_utf8_lossy(st.firmware_vendor().as_bytes()).into_owned();

    let memory = get_memory_info(bt)?;
    let devices = detect_storage_devices(bt)?;

    Ok(SystemInfo {
        uefi_version: (revision.major(), revision.minor()),
        firmware_vendor,
        firmware_revision: 0, // TODO
        memory,
        storage_device_count: devices.len(),
    })
}

/// 延时 (毫秒)
pub fn delay_ms(bt: &BootServices, milliseconds: u64) {
    use uefi::table::boot::TimerTrigger;

    let event = unsafe {
        bt.create_event(
            uefi::table::boot::EventType::TIMER,
            uefi::table::boot::Tpl::APPLICATION,
            None,
            None,
        )
    }
    .unwrap();
    bt.set_timer(&event, TimerTrigger::Relative(10_000_000 * milliseconds))
        .ok();
    let mut events = [event];
    bt.wait_for_event(&mut events).ok();
}

/// 延时 (秒)
pub fn delay_seconds(bt: &BootServices, seconds: u64) {
    delay_ms(bt, seconds * 1000);
}
