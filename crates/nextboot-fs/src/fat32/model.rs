/// FAT32 引导扇区
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub(super) struct Fat32BootSector {
    pub(super) jump: [u8; 3],
    pub(super) oem: [u8; 8],
    pub(super) bytes_per_sector: u16,
    pub(super) sectors_per_cluster: u8,
    pub(super) reserved_sectors: u16,
    pub(super) num_fats: u8,
    pub(super) root_entries: u16,
    pub(super) total_sectors_16: u16,
    pub(super) media_type: u8,
    pub(super) sectors_per_fat_16: u16,
    pub(super) sectors_per_track: u16,
    pub(super) num_heads: u16,
    pub(super) hidden_sectors: u32,
    pub(super) total_sectors_32: u32,
    // FAT32 扩展
    pub(super) sectors_per_fat_32: u32,
    pub(super) ext_flags: u16,
    pub(super) fs_version: u16,
    pub(super) root_cluster: u32,
    pub(super) fs_info_sector: u16,
    pub(super) backup_boot_sector: u16,
    pub(super) reserved: [u8; 12],
    pub(super) drive_num: u8,
    pub(super) reserved1: u8,
    pub(super) boot_signature: u8,
    pub(super) volume_id: u32,
    pub(super) volume_label: [u8; 11],
    pub(super) fs_type: [u8; 8],
}

/// FAT 目录条目 (32 字节)
#[repr(C, packed)]
struct FatDirEntry {
    pub(super) name: [u8; 11],
    pub(super) attr: u8,
    pub(super) nt_reserved: u8,
    pub(super) create_time_tenth: u8,
    pub(super) create_time: u16,
    pub(super) create_date: u16,
    pub(super) last_access_date: u16,
    pub(super) cluster_high: u16,
    pub(super) modify_time: u16,
    pub(super) modify_date: u16,
    pub(super) cluster_low: u16,
    pub(super) file_size: u32,
}

/// 长文件名条目
struct LfnEntry {
    pub(super) seq: u8,
    pub(super) name1: [u16; 5],
    pub(super) attr: u8,
    pub(super) type_: u8,
    pub(super) checksum: u8,
    pub(super) name2: [u16; 6],
    pub(super) reserved: u16,
    pub(super) name3: [u16; 2],
}
