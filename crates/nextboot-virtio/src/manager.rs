use alloc::vec::Vec;

use crate::VirtualBlockIo;

/// 虚拟设备管理器
pub struct VirtualDeviceManager {
    /// 已注册的虚拟设备
    devices: Vec<VirtualBlockIo>,
    /// 下一个设备索引
    next_index: usize,
}

impl VirtualDeviceManager {
    /// 创建新的设备管理器
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            next_index: 0,
        }
    }

    /// 注册新的虚拟设备
    pub fn register(&mut self, device: VirtualBlockIo) -> usize {
        let index = self.next_index;
        self.devices.push(device);
        self.next_index += 1;
        index
    }

    /// 获取设备
    pub fn get(&self, index: usize) -> Option<&VirtualBlockIo> {
        self.devices.get(index)
    }

    /// 获取可变引用
    pub fn get_mut(&mut self, index: usize) -> Option<&mut VirtualBlockIo> {
        self.devices.get_mut(index)
    }

    /// 获取设备数量
    pub fn count(&self) -> usize {
        self.devices.len()
    }

    /// 移除设备
    pub fn remove(&mut self, index: usize) -> Option<VirtualBlockIo> {
        if index < self.devices.len() {
            Some(self.devices.remove(index))
        } else {
            None
        }
    }
}

impl Default for VirtualDeviceManager {
    fn default() -> Self {
        Self::new()
    }
}
