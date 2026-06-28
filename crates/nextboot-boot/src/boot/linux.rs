use super::candidates::{
    generic_efi_boot_paths, LINUX_CONFIG_MAX_SIZE, LINUX_GRUB_CONFIG_CANDIDATES,
    LINUX_INITRD_CANDIDATES, LINUX_ISOLINUX_CONFIG_CANDIDATES, LINUX_KERNEL_CANDIDATES,
    LINUX_LOADER_ENTRY_DIRS,
};
use super::load_file::LinuxInitrdLoadFile2Protocol;
use super::util::{
    is_isolinux_config_path, iso_parent_dir, push_unique_iso_path, resolve_linux_config_path,
};
use super::{BootManager, VirtualBootDevice};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use log::{info, warn};
use nextboot_linux::{
    parse_grub_boot_entry, parse_isolinux_boot_entry, EfiStubOptions, LinuxBootConfig,
    LinuxBootEntry, LinuxBootloader, LinuxDistro,
};
use uefi::Status;

impl BootManager<'_> {
    /// 引导 Linux ISO
    pub(super) fn boot_linux(&self, device: &VirtualBootDevice) -> uefi::Result<()> {
        info!("Booting Linux ISO...");
        if let Ok(()) = self.try_chain_load_paths(device, generic_efi_boot_paths()) {
            return Ok(());
        }

        let distro = match self.iso.os_type {
            crate::scanner::OsType::Ubuntu => LinuxDistro::Ubuntu,
            crate::scanner::OsType::Debian => LinuxDistro::Debian,
            crate::scanner::OsType::Fedora => LinuxDistro::Fedora,
            crate::scanner::OsType::Arch => LinuxDistro::Arch,
            _ => LinuxDistro::Generic,
        };

        let config = self.discover_linux_boot_config(distro)?;

        info!("Kernel: {}", config.kernel_path);
        info!("Initrd: {}", config.initrd_path);
        if config.initrd_paths.len() > 1 {
            for (index, path) in config.initrd_paths.iter().enumerate() {
                info!("Initrd[{}]: {}", index, path);
            }
        }
        info!("Cmdline: {}", config.cmdline);

        let mut bootloader = LinuxBootloader::new(config);
        let kernel_data = self.load_file(&bootloader.config().kernel_path)?;
        bootloader
            .load_kernel(kernel_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        let mut initrd_data = self.load_linux_initrd_chain(&bootloader.config().initrd_paths)?;
        if let Err(err) = self.append_ventoy_linux_initrd_overlay(&mut initrd_data) {
            warn!(
                "Failed to append Ventoy Linux initrd overlay for {}: {:?}",
                self.iso.path,
                err.status()
            );
        }
        bootloader
            .load_initrd(initrd_data)
            .map_err(|_| Status::LOAD_ERROR)?;

        let kernel_size = bootloader.kernel_size();
        let initrd_size = bootloader.initrd_size();
        let (config, kernel_data, initrd_data) = bootloader.into_parts();
        drop(kernel_data);

        let load_options =
            EfiStubOptions::new(&config.cmdline, &config.initrd_path).to_load_option_string();
        info!(
            "Prepared Linux EFI stub: kernel={} bytes initrd={} bytes options={}",
            kernel_size, initrd_size, load_options
        );

        match LinuxInitrdLoadFile2Protocol::install(self.bt, initrd_data) {
            Ok(provider) => {
                info!(
                    "Registered Linux EFI initrd LoadFile2 provider: {} bytes",
                    initrd_size
                );
                provider.leak();
            }
            Err(err) => warn!(
                "Failed to register Linux EFI initrd LoadFile2 provider: {:?}; falling back to initrd path load option",
                err.status()
            ),
        }

        self.load_image_from_device_path_with_options(
            device.handle,
            &device.device_path,
            &config.kernel_path,
            "Linux EFI stub",
            Some(&load_options),
        )
    }

    fn discover_linux_boot_config(&self, distro: LinuxDistro) -> uefi::Result<LinuxBootConfig> {
        if let Some(config) = self.discover_linux_config_file_boot_config(distro)? {
            return Ok(config);
        }

        if let Some(config) = self.discover_linux_candidate_boot_config(distro)? {
            return Ok(config);
        }

        let config = LinuxBootConfig::for_distro(distro, &self.iso.path);
        warn!(
            "Falling back to built-in Linux boot paths for {}: kernel={} initrd={}",
            self.iso.path, config.kernel_path, config.initrd_path
        );
        Ok(config)
    }

    fn discover_linux_config_file_boot_config(
        &self,
        distro: LinuxDistro,
    ) -> uefi::Result<Option<LinuxBootConfig>> {
        let mut config_paths = Vec::new();
        for path in LINUX_GRUB_CONFIG_CANDIDATES
            .iter()
            .chain(LINUX_ISOLINUX_CONFIG_CANDIDATES.iter())
        {
            push_unique_iso_path(&mut config_paths, path)?;
        }
        self.discover_linux_loader_entry_configs(&mut config_paths)?;

        for path in config_paths {
            let text = match self.load_iso_text_file(&path, LINUX_CONFIG_MAX_SIZE) {
                Ok(text) => text,
                Err(err) if err.status() == Status::NOT_FOUND => continue,
                Err(err) => {
                    warn!(
                        "Linux config candidate {} was not loaded: {:?}",
                        path,
                        err.status()
                    );
                    continue;
                }
            };

            let parsed = if is_isolinux_config_path(&path) {
                parse_isolinux_boot_entry(&text).or_else(|| parse_grub_boot_entry(&text))
            } else {
                parse_grub_boot_entry(&text).or_else(|| parse_isolinux_boot_entry(&text))
            };
            let Some(entry) = parsed else {
                info!(
                    "Linux config {} did not contain a complete boot entry",
                    path
                );
                continue;
            };

            let base_dir = iso_parent_dir(&path);
            let kernel_path = resolve_linux_config_path(&base_dir, &entry.kernel_path);
            let initrd_paths = self.resolve_linux_initrd_paths(&base_dir, &entry)?;
            if !self.iso_file_exists(&kernel_path)? {
                warn!(
                    "Linux config {} references missing kernel {}",
                    path, kernel_path
                );
                continue;
            }

            let mut missing_initrd = None;
            for initrd_path in &initrd_paths {
                if !self.iso_file_exists(initrd_path)? {
                    missing_initrd = Some(initrd_path.clone());
                    break;
                }
            }
            if let Some(missing) = missing_initrd {
                warn!(
                    "Linux config {} references missing initrd {}",
                    path, missing
                );
                continue;
            }

            let initrd_summary = linux_initrd_summary(&initrd_paths);
            info!(
                "Discovered Linux boot config from {}: kernel={} initrd={} cmdline={}",
                path, kernel_path, initrd_summary, entry.cmdline
            );
            return Ok(Some(LinuxBootConfig::from_initrd_paths(
                distro,
                &self.iso.path,
                &kernel_path,
                initrd_paths,
                &entry.cmdline,
            )));
        }

        Ok(None)
    }

    fn discover_linux_loader_entry_configs(&self, paths: &mut Vec<String>) -> uefi::Result<()> {
        for dir in LINUX_LOADER_ENTRY_DIRS {
            let entries = match self.read_iso_dir(dir) {
                Ok(entries) => entries,
                Err(err) if err.status() == Status::NOT_FOUND => continue,
                Err(err) => {
                    warn!(
                        "Linux loader entry dir {} was not scanned: {:?}",
                        dir,
                        err.status()
                    );
                    continue;
                }
            };

            for entry in entries {
                if entry.is_dir || !entry.name.to_ascii_lowercase().ends_with(".conf") {
                    continue;
                }

                push_unique_iso_path(paths, &format!("{}/{}", dir, entry.name))?;
            }
        }

        Ok(())
    }

    fn resolve_linux_initrd_paths(
        &self,
        base_dir: &str,
        entry: &LinuxBootEntry,
    ) -> uefi::Result<Vec<String>> {
        let mut paths = Vec::new();
        paths
            .try_reserve_exact(entry.initrd_paths.len())
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        for initrd in &entry.initrd_paths {
            paths.push(resolve_linux_config_path(base_dir, initrd));
        }
        Ok(paths)
    }

    fn discover_linux_candidate_boot_config(
        &self,
        distro: LinuxDistro,
    ) -> uefi::Result<Option<LinuxBootConfig>> {
        let default = LinuxBootConfig::for_distro(distro, &self.iso.path);
        if self.iso_file_exists(&default.kernel_path)?
            && self.iso_file_exists(&default.initrd_path)?
        {
            info!(
                "Using distro Linux defaults: kernel={} initrd={}",
                default.kernel_path, default.initrd_path
            );
            return Ok(Some(default));
        }

        let Some(kernel_path) = self.first_existing_iso_file(LINUX_KERNEL_CANDIDATES)? else {
            warn!(
                "No Linux kernel candidate found in {}; tried {:?}",
                self.iso.path, LINUX_KERNEL_CANDIDATES
            );
            return Ok(None);
        };
        let Some(initrd_path) = self.first_existing_iso_file(LINUX_INITRD_CANDIDATES)? else {
            warn!(
                "Linux kernel candidate {} was found in {}, but no initrd candidate was found; tried {:?}",
                kernel_path, self.iso.path, LINUX_INITRD_CANDIDATES
            );
            return Ok(None);
        };

        info!(
            "Using Ventoy-style Linux candidates: kernel={} initrd={}",
            kernel_path, initrd_path
        );
        Ok(Some(LinuxBootConfig::from_paths(
            distro,
            &self.iso.path,
            &kernel_path,
            &initrd_path,
            &default.cmdline,
        )))
    }

    fn load_linux_initrd_chain(&self, paths: &[String]) -> uefi::Result<Vec<u8>> {
        let mut combined = Vec::new();
        for path in paths {
            let data = self.load_file(path)?;
            if data.is_empty() {
                return Err(Status::LOAD_ERROR.into());
            }

            let offset = combined.len();
            combined
                .try_reserve(data.len())
                .map_err(|_| Status::OUT_OF_RESOURCES)?;
            combined.extend_from_slice(&data);
            info!(
                "Loaded Linux initrd component {}: {} bytes at offset {}",
                path,
                data.len(),
                offset
            );
        }

        if combined.is_empty() {
            return Err(Status::NOT_FOUND.into());
        }

        Ok(combined)
    }
}

fn linux_initrd_summary(paths: &[String]) -> String {
    let mut out = String::new();
    for (index, path) in paths.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(path);
    }

    out
}
