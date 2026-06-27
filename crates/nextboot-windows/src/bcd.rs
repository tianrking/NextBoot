use alloc::string::String;
use alloc::vec::Vec;

/// BCD 对象类型
#[derive(Debug, Clone, Copy)]
pub enum BcdObjectType {
    Application,
    Device,
    Inherit,
    Library,
}

/// 简单的 BCD 解析器
pub fn parse_bcd(data: &[u8]) -> Option<BcdStore> {
    // BCD 是注册表格式的 hive 文件
    // 简化实现: 只解析基本结构

    if data.len() < 4 {
        return None;
    }

    // 检查注册表签名 "regf"
    if &data[0..4] != b"regf" {
        return None;
    }

    Some(BcdStore { _data: Vec::new() })
}

/// BCD 存储
pub struct BcdStore {
    _data: Vec<u8>,
}

impl BcdStore {
    /// 获取默认启动项
    pub fn get_default_entry(&self) -> Option<u64> {
        // TODO: 解析 BCD 获取 {default} GUID
        None
    }

    /// 获取启动项描述
    pub fn get_entry_description(&self, _id: u64) -> Option<String> {
        None
    }
}

/// BCD 元素类型
#[derive(Debug, Clone, Copy)]
pub enum BcdElementType {
    /// 应用路径
    ApplicationPath = 0x1200002,
    /// 设备
    OsDevice = 0x2100001,
    /// OS 文件设备
    OsFileDevice = 0x2200002,
    /// 描述
    Description = 0x1200004,
}
