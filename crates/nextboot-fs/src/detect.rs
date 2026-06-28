use crate::FileSystemType;

/// ISO 镜像类型检测
pub fn detect_iso_type(data: &[u8]) -> FileSystemType {
    // ISO9660 检测: 卷描述符位于第 16 个逻辑扇区
    if data.len() >= 0x8000 + 6 {
        let vd = &data[0x8000..];
        if &vd[1..6] == b"CD001" {
            return FileSystemType::Iso9660;
        }
    }

    FileSystemType::Unknown
}

/// 检测文件系统类型
pub fn detect_fs_type(data: &[u8]) -> FileSystemType {
    // FAT32 检测
    if data.len() >= 510 {
        // 检查引导签名
        if data[510] == 0x55 && data[511] == 0xAA {
            // FAT32 filesystem type is an 8-byte field at offset 0x52.
            if data.len() >= 0x5A && data[0x52..0x5A].starts_with(b"FAT32") {
                return FileSystemType::Fat32;
            }
            // FAT12/16 签名
            if data.len() >= 0x08 && &data[0x03..0x08] == b"FAT12" {
                return FileSystemType::Fat32; // 简化处理
            }
            if data.len() >= 0x08 && &data[0x03..0x08] == b"FAT16" {
                return FileSystemType::Fat32; // 简化处理
            }
        }
    }

    // exFAT 检测
    if data.len() >= 3 {
        // exFAT 跳转指令和签名
        if data[0] == 0xEB && data[1] == 0x76 && data[2] == 0x90 {
            // 完整签名在偏移 3: "EXFAT"
            if data.len() >= 11 && &data[3..11] == b"EXFAT   " {
                return FileSystemType::ExFat;
            }
        }
    }

    // NTFS 检测
    if data.len() >= 11 && &data[3..11] == b"NTFS    " {
        return FileSystemType::Ntfs;
    }

    // XFS 检测
    if data.len() >= 4 && &data[0..4] == b"XFSB" {
        return FileSystemType::Xfs;
    }

    // ISO9660 检测
    detect_iso_type(data)
}
