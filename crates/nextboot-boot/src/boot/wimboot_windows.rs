use super::candidates::VTOYJUMP_CANDIDATES;
use super::errors::{
    ventoy_windows_runtime_data_error_to_uefi_status,
    ventoy_windows_wimboot_payload_error_to_uefi_status,
};
use super::BootManager;
use alloc::vec::Vec;
use log::info;
use nextboot_virtio::VirtualDeviceConfig;
use uefi::Status;

impl BootManager<'_> {
    pub(super) fn prepare_windows_wimboot_jump_payload(
        &self,
        boot_config: &VirtualDeviceConfig,
        original_winpeshl: &[u8],
    ) -> uefi::Result<Option<Vec<u8>>> {
        let jump = match self.find_source_volume_file(VTOYJUMP_CANDIDATES, &[]) {
            Ok(file) => file,
            Err(err) if err.status() == Status::NOT_FOUND => {
                info!(
                    "Ventoy Windows jump helper was not found for {}; skipping winpeshl.exe overlay",
                    self.iso.path
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

        let plugin = self.iso.ventoy_plugin.as_ref();
        let auto_install = self.load_selected_auto_install_template(plugin)?;
        let injection = self.load_plugin_injection_archive(plugin)?;
        let runtime_input = nextboot_windows::VentoyWindowsRuntimeDataInput {
            auto_install: auto_install.as_ref().map(|file| {
                nextboot_windows::VentoyWindowsAutoInstall {
                    source_path: file.path.as_str(),
                    data: file.data.as_slice(),
                }
            }),
            injection_archive: injection.as_ref().map(|file| file.path.as_str()),
            windows11_bypass_check: self.iso.ventoy_windows11_bypass_check,
            windows11_bypass_nro: self.iso.ventoy_windows11_bypass_nro,
        };
        let windows_data = nextboot_windows::build_ventoy_windows_runtime_data(runtime_input)
            .map_err(ventoy_windows_runtime_data_error_to_uefi_status)?;
        let (os_param, image_chunks, image_location_addr) =
            self.build_ventoy_os_param_payload(boot_config)?;
        let payload = nextboot_windows::build_ventoy_wimboot_jump_payload(
            jump.data.as_slice(),
            &os_param,
            windows_data.as_slice(),
            original_winpeshl,
        )
        .map_err(ventoy_windows_wimboot_payload_error_to_uefi_status)?;

        info!(
            "Built Ventoy Windows runtime data for {}: jump={}, auto_install={}, injection={}, win11_bypass_check={}, win11_bypass_nro={}, image_chunks={}, image_location=0x{:x}",
            self.iso.path,
            jump.path,
            auto_install.is_some(),
            injection.is_some(),
            self.iso.ventoy_windows11_bypass_check,
            self.iso.ventoy_windows11_bypass_nro,
            image_chunks,
            image_location_addr
        );

        Ok(Some(payload))
    }
}
