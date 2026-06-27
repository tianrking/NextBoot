use super::candidates::{default_efi_boot_paths, generic_efi_boot_paths};
use super::errors::{fs_error_to_uefi_status, virtio_error_to_fs_error};
use super::load_file::{normalize_load_file_key, PreloadedFile};
use super::util::{device_path_to_vec, is_child_device_path};
use super::{BootManager, VirtualBootDevice};
use crate::virtual_fs::{
    IsoSimpleFileSystemProtocol, RegisteredIsoSimpleFileSystem, VirtualFileReplacement,
    VirtualIsoFilesystem,
};
use alloc::rc::Rc;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_fs::iso9660::Iso9660;
use nextboot_fs::udf::Udf;
use nextboot_fs::{BlockIoOps, FsError};
use nextboot_virtio::VirtualBlockIo;
use uefi::proto::device_path::DevicePath;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::table::boot::{OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::{Handle, Identify, Status};

impl BootManager<'_> {
    pub(super) fn open_iso9660_filesystem(
        &self,
        source_block_io: &BlockIO,
    ) -> uefi::Result<Iso9660> {
        let config = self.iso9660_virtual_config();
        let vbio = self.build_source_backed_virtual_block_io(config, source_block_io)?;
        Ok(
            Iso9660::open(Rc::new(VirtualIsoBlockIo::new(vbio)))
                .map_err(fs_error_to_uefi_status)?,
        )
    }

    pub(super) fn open_udf_filesystem(&self, source_block_io: &BlockIO) -> uefi::Result<Udf> {
        let config = self.iso9660_virtual_config();
        let vbio = self.build_source_backed_virtual_block_io(config, source_block_io)?;
        Ok(Udf::open(Rc::new(VirtualIsoBlockIo::new(vbio))).map_err(fs_error_to_uefi_status)?)
    }

    pub(super) fn open_virtual_iso_filesystem(
        &self,
        source_block_io: &BlockIO,
    ) -> uefi::Result<VirtualIsoFilesystem> {
        let config = self.iso9660_virtual_config();
        if self.iso.is_udf {
            let vbio =
                self.build_source_backed_virtual_block_io(config.clone(), source_block_io)?;
            match Udf::open(Rc::new(VirtualIsoBlockIo::new(vbio))) {
                Ok(udf) => {
                    info!("Using UDF SimpleFileSystem backend for {}", self.iso.path);
                    return Ok(VirtualIsoFilesystem::Udf(udf));
                }
                Err(err) => {
                    warn!(
                        "Failed to open UDF filesystem for {}, falling back to ISO9660: {:?}",
                        self.iso.path, err
                    );
                }
            }
        }

        let vbio = self.build_source_backed_virtual_block_io(config, source_block_io)?;
        let iso = Iso9660::open(Rc::new(VirtualIsoBlockIo::new(vbio)))
            .map_err(fs_error_to_uefi_status)?;
        Ok(VirtualIsoFilesystem::Iso9660(iso))
    }

    pub(super) fn install_iso_simple_file_system(
        &self,
        source_block_io: &BlockIO,
        virtual_handle: Handle,
        replacements: Vec<VirtualFileReplacement>,
    ) -> uefi::Result<RegisteredIsoSimpleFileSystem> {
        let fs = self.open_virtual_iso_filesystem(source_block_io)?;
        let block_size = fs.block_size();
        IsoSimpleFileSystemProtocol::install(
            self.bt,
            virtual_handle,
            Rc::new(fs),
            self.iso.size,
            block_size,
            replacements,
        )
    }

    pub(super) fn preload_load_file_entries(&self) -> Vec<PreloadedFile> {
        let mut entries = Vec::new();
        if !self.iso.image_format.is_iso() {
            return entries;
        }

        for path in generic_efi_boot_paths() {
            match self.load_file(path) {
                Ok(data) if !data.is_empty() => {
                    let key = normalize_load_file_key(path);
                    if entries
                        .iter()
                        .any(|entry: &PreloadedFile| entry.path == key)
                    {
                        continue;
                    }

                    info!("Preloaded LoadFile path {} ({} bytes)", path, data.len());
                    entries.push(PreloadedFile { path: key, data });
                }
                Ok(_) => {}
                Err(err) => {
                    info!("LoadFile preload skipped {}: {:?}", path, err.status());
                }
            }
        }

        entries
    }

    pub(super) fn boot_virtual_device(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Connecting virtual boot device {:?}", device.handle);
        if let Err(err) = self.bt.connect_controller(device.handle, None, None, true) {
            warn!(
                "ConnectController on virtual device returned {:?}; trying LoadImage anyway",
                err.status()
            );
        }

        if !self.iso.image_format.is_iso() {
            match self.boot_virtual_disk_partitions(device) {
                Ok(()) => return Ok(()),
                Err(err) => warn!(
                    "Virtual disk partition boot failed for {}: {:?}",
                    self.iso.path,
                    err.status()
                ),
            }
        }

        self.try_load_image_paths(
            device.handle,
            &device.device_path,
            default_efi_boot_paths(),
            "virtual device",
        )
    }

    fn boot_virtual_disk_partitions(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        let mut last_status = Status::NOT_FOUND;
        for attempt in 0..3 {
            let fs_handles = self
                .bt
                .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))?;
            let mut matched_partitions = 0usize;

            for handle in fs_handles.iter().copied() {
                let Ok(partition_path) = self.handle_device_path_bytes(handle) else {
                    continue;
                };

                if !is_child_device_path(&device.device_path, &partition_path) {
                    continue;
                }

                matched_partitions += 1;
                info!(
                    "Found virtual disk filesystem partition {:?} for {}",
                    handle, self.iso.path
                );

                match self.try_load_image_paths(
                    handle,
                    &partition_path,
                    default_efi_boot_paths(),
                    "virtual disk partition",
                ) {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        last_status = err.status();
                        warn!(
                            "No bootable EFI path on virtual partition {:?}: {:?}",
                            handle,
                            err.status()
                        );
                    }
                }
            }

            if matched_partitions > 0 {
                return Err(last_status.into());
            }

            warn!(
                "No SimpleFileSystem partitions found under {} (attempt {}/3)",
                self.iso.path,
                attempt + 1
            );
            self.bt.stall(2_000_000);
        }

        Err(last_status.into())
    }

    pub(super) fn handle_device_path_bytes(&self, handle: Handle) -> uefi::Result<Vec<u8>> {
        let device_path = unsafe {
            self.bt.open_protocol::<DevicePath>(
                OpenProtocolParams {
                    handle,
                    agent: self.parent_image,
                    controller: None,
                },
                OpenProtocolAttributes::GetProtocol,
            )
        }?;

        device_path_to_vec(&device_path)
    }
}

struct VirtualIsoBlockIo {
    vbio: VirtualBlockIo,
    media_id: u32,
}

impl VirtualIsoBlockIo {
    fn new(vbio: VirtualBlockIo) -> Self {
        let media_id = vbio.media_id();
        Self { vbio, media_id }
    }
}

impl BlockIoOps for VirtualIsoBlockIo {
    fn block_size(&self) -> u32 {
        self.vbio.block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.vbio.block_count()
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.vbio
            .read_blocks(self.media_id, lba, buf)
            .map_err(virtio_error_to_fs_error)
    }
}
