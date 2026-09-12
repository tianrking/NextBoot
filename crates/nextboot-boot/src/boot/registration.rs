use super::load_file::RegisteredPreloadedLoadFile;
use crate::virtual_fs::RegisteredIsoSimpleFileSystem;
use core::cell::Cell;
use log::{info, warn};
use nextboot_virtio::protocol::RegisteredVirtualBlockIo;
use uefi::table::boot::BootServices;

/// Keeps firmware interfaces alive through StartImage, then releases them in dependency order.
pub(super) struct VirtualRegistration<'a> {
    pub(super) bt: &'a BootServices,
    pub(super) cleanup_ok: &'a Cell<bool>,
    pub(super) block: Option<RegisteredVirtualBlockIo>,
    pub(super) filesystem: Option<RegisteredIsoSimpleFileSystem>,
    pub(super) files: Option<RegisteredPreloadedLoadFile>,
}

impl Drop for VirtualRegistration<'_> {
    fn drop(&mut self) {
        let Some(block) = self.block.take() else { return };
        let handle = block.handle();
        let mut ok = true;
        info!("Releasing virtual device {:?}: disconnecting firmware drivers", handle);
        if let Err(error) = self.bt.disconnect_controller(handle, None, None) {
            warn!("Virtual controller disconnect failed: {:?}", error.status());
            ok = false;
        }
        if let Some(files) = self.files.take() {
            info!("Releasing virtual device: preloaded file protocols");
            ok &= files.uninstall(self.bt, handle).is_ok();
        }
        if let Some(filesystem) = self.filesystem.take() {
            info!("Releasing virtual device: filesystem protocol");
            ok &= filesystem.uninstall(self.bt, handle).is_ok();
        }
        info!("Releasing virtual device: block and disk protocols");
        ok &= block.uninstall(self.bt).is_ok();
        if ok {
            info!("Released virtual boot device after loader returned");
        } else {
            self.cleanup_ok.set(false);
            warn!("Virtual boot device cleanup incomplete; further boot attempts are disabled");
        }
    }
}
