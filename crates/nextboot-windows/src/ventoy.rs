use alloc::vec::Vec;

pub const VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE: usize = 384;
pub const VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE: usize = 384;
pub const VENTOY_WINDOWS_DATA_RESERVED_SIZE: usize = 250;
pub const VENTOY_WINDOWS_DATA_HEADER_SIZE: usize = 1024;

const VENTOY_WIMBOOT_JUMP_ALIGNMENT: usize = 16;
const VENTOY_WIMBOOT_PAYLOAD_ALIGNMENT: usize = 2048;
const AUTO_INSTALL_SCRIPT_OFFSET: usize = 0;
const INJECTION_ARCHIVE_OFFSET: usize =
    AUTO_INSTALL_SCRIPT_OFFSET + VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE;
const WINDOWS11_BYPASS_CHECK_OFFSET: usize =
    INJECTION_ARCHIVE_OFFSET + VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE;
const AUTO_INSTALL_LEN_OFFSET: usize = WINDOWS11_BYPASS_CHECK_OFFSET + 1;
const WINDOWS11_BYPASS_NRO_OFFSET: usize = AUTO_INSTALL_LEN_OFFSET + 4;
const RESERVED_OFFSET: usize = WINDOWS11_BYPASS_NRO_OFFSET + 1;

/// Ventoy Windows auto-install payload appended after `ventoy_windows_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VentoyWindowsAutoInstall<'a> {
    /// Original template path from `ventoy.json`.
    pub source_path: &'a str,
    /// Template file bytes appended after the 1024-byte runtime header.
    pub data: &'a [u8],
}

/// Input for building Ventoy-compatible Windows runtime data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VentoyWindowsRuntimeDataInput<'a> {
    pub auto_install: Option<VentoyWindowsAutoInstall<'a>>,
    pub injection_archive: Option<&'a str>,
    pub windows11_bypass_check: bool,
    pub windows11_bypass_nro: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyWindowsRuntimeDataError {
    AutoInstallTooLarge,
    OutputReserveFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VentoyWindowsWimbootPayloadError {
    OutputReserveFailed,
}

/// Build Ventoy's packed `ventoy_windows_data` buffer.
///
/// Ventoy injects this blob through its modified wimboot helper. The header is
/// packed to 1024 bytes and any selected auto-install template bytes are
/// appended directly after it.
pub fn build_ventoy_windows_runtime_data(
    input: VentoyWindowsRuntimeDataInput<'_>,
) -> Result<Vec<u8>, VentoyWindowsRuntimeDataError> {
    debug_assert_eq!(RESERVED_OFFSET + VENTOY_WINDOWS_DATA_RESERVED_SIZE, 1024);

    let auto_install_len = input
        .auto_install
        .as_ref()
        .map_or(0usize, |auto_install| auto_install.data.len());
    if auto_install_len > u32::MAX as usize {
        return Err(VentoyWindowsRuntimeDataError::AutoInstallTooLarge);
    }

    let total_size = VENTOY_WINDOWS_DATA_HEADER_SIZE
        .checked_add(auto_install_len)
        .ok_or(VentoyWindowsRuntimeDataError::OutputReserveFailed)?;
    let mut out = Vec::new();
    out.try_reserve_exact(total_size)
        .map_err(|_| VentoyWindowsRuntimeDataError::OutputReserveFailed)?;
    out.resize(VENTOY_WINDOWS_DATA_HEADER_SIZE, 0);

    if let Some(auto_install) = input.auto_install {
        copy_ventoy_c_string(
            &mut out[AUTO_INSTALL_SCRIPT_OFFSET..INJECTION_ARCHIVE_OFFSET],
            ventoy_basename(auto_install.source_path),
        );
        out[AUTO_INSTALL_LEN_OFFSET..WINDOWS11_BYPASS_NRO_OFFSET]
            .copy_from_slice(&(auto_install.data.len() as u32).to_le_bytes());
        out.extend_from_slice(auto_install.data);
    }

    if let Some(injection_archive) = input.injection_archive {
        copy_ventoy_c_string(
            &mut out[INJECTION_ARCHIVE_OFFSET..WINDOWS11_BYPASS_CHECK_OFFSET],
            injection_archive,
        );
    }

    out[WINDOWS11_BYPASS_CHECK_OFFSET] = u8::from(input.windows11_bypass_check);
    out[WINDOWS11_BYPASS_NRO_OFFSET] = u8::from(input.windows11_bypass_nro);

    Ok(out)
}

/// Build Ventoy's WIMBOOT `winpeshl.exe` replacement payload.
///
/// Ventoy prepends `vtoyjump*.exe`, then embeds `VentoyOsParam`,
/// `ventoy_windows_data`, and the original WinPE `winpeshl.exe`.
pub fn build_ventoy_wimboot_jump_payload(
    jump_exe: &[u8],
    os_param: &[u8],
    windows_data: &[u8],
    original_exe: &[u8],
) -> Result<Vec<u8>, VentoyWindowsWimbootPayloadError> {
    let jump_align = align_up(jump_exe.len(), VENTOY_WIMBOOT_JUMP_ALIGNMENT)
        .ok_or(VentoyWindowsWimbootPayloadError::OutputReserveFailed)?;
    let raw_len = jump_align
        .checked_add(os_param.len())
        .and_then(|len| len.checked_add(windows_data.len()))
        .and_then(|len| len.checked_add(original_exe.len()))
        .ok_or(VentoyWindowsWimbootPayloadError::OutputReserveFailed)?;
    let aligned_len = align_up(raw_len, VENTOY_WIMBOOT_PAYLOAD_ALIGNMENT)
        .ok_or(VentoyWindowsWimbootPayloadError::OutputReserveFailed)?;

    let mut out = Vec::new();
    out.try_reserve_exact(aligned_len)
        .map_err(|_| VentoyWindowsWimbootPayloadError::OutputReserveFailed)?;
    out.resize(aligned_len, 0);
    out[..jump_exe.len()].copy_from_slice(jump_exe);
    let os_param_offset = jump_align;
    out[os_param_offset..os_param_offset + os_param.len()].copy_from_slice(os_param);
    let windows_data_offset = os_param_offset + os_param.len();
    out[windows_data_offset..windows_data_offset + windows_data.len()]
        .copy_from_slice(windows_data);
    let original_exe_offset = windows_data_offset + windows_data.len();
    out[original_exe_offset..original_exe_offset + original_exe.len()]
        .copy_from_slice(original_exe);
    Ok(out)
}

fn copy_ventoy_c_string(field: &mut [u8], value: &str) {
    if field.is_empty() {
        return;
    }

    let bytes = value.as_bytes();
    let copy_len = core::cmp::min(bytes.len(), field.len() - 1);
    field[..copy_len].copy_from_slice(&bytes[..copy_len]);
}

fn ventoy_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    if !alignment.is_power_of_two() {
        return None;
    }
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn encodes_bypass_flags_and_plugin_metadata() {
        let payload = b"answer";
        let data = build_ventoy_windows_runtime_data(VentoyWindowsRuntimeDataInput {
            auto_install: Some(VentoyWindowsAutoInstall {
                source_path: "/answer/autounattend.xml",
                data: payload,
            }),
            injection_archive: Some("/ventoy/inject.zip"),
            windows11_bypass_check: true,
            windows11_bypass_nro: true,
        })
        .expect("runtime data");

        let script = b"autounattend.xml";
        let injection = b"/ventoy/inject.zip";

        assert_eq!(data.len(), VENTOY_WINDOWS_DATA_HEADER_SIZE + payload.len());
        assert_eq!(&data[..script.len()], script);
        assert_eq!(data[script.len()], 0);
        assert_eq!(
            &data[INJECTION_ARCHIVE_OFFSET..INJECTION_ARCHIVE_OFFSET + injection.len()],
            injection
        );
        assert_eq!(data[INJECTION_ARCHIVE_OFFSET + injection.len()], 0);
        assert_eq!(data[WINDOWS11_BYPASS_CHECK_OFFSET], 1);
        assert_eq!(
            u32::from_le_bytes(
                data[AUTO_INSTALL_LEN_OFFSET..WINDOWS11_BYPASS_NRO_OFFSET]
                    .try_into()
                    .unwrap()
            ),
            payload.len() as u32
        );
        assert_eq!(data[WINDOWS11_BYPASS_NRO_OFFSET], 1);
        assert_eq!(&data[VENTOY_WINDOWS_DATA_HEADER_SIZE..], payload);
    }

    #[test]
    fn truncates_c_string_fields_like_ventoy() {
        let mut auto_path = String::from("/");
        let mut injection = String::new();
        for _ in 0..400 {
            auto_path.push('a');
            injection.push('b');
        }

        let data = build_ventoy_windows_runtime_data(VentoyWindowsRuntimeDataInput {
            auto_install: Some(VentoyWindowsAutoInstall {
                source_path: &auto_path,
                data: b"x",
            }),
            injection_archive: Some(&injection),
            ..VentoyWindowsRuntimeDataInput::default()
        })
        .expect("runtime data");

        assert!(data[..VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE - 1]
            .iter()
            .all(|byte| *byte == b'a'));
        assert_eq!(data[VENTOY_WINDOWS_DATA_AUTO_INSTALL_SCRIPT_SIZE - 1], 0);
        assert!(data[INJECTION_ARCHIVE_OFFSET
            ..INJECTION_ARCHIVE_OFFSET + VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE - 1]
            .iter()
            .all(|byte| *byte == b'b'));
        assert_eq!(
            data[INJECTION_ARCHIVE_OFFSET + VENTOY_WINDOWS_DATA_INJECTION_ARCHIVE_SIZE - 1],
            0
        );
    }

    #[test]
    fn omits_auto_install_payload_when_absent() {
        let data = build_ventoy_windows_runtime_data(VentoyWindowsRuntimeDataInput::default())
            .expect("runtime data");

        assert_eq!(data.len(), VENTOY_WINDOWS_DATA_HEADER_SIZE);
        assert_eq!(data[WINDOWS11_BYPASS_CHECK_OFFSET], 0);
        assert_eq!(
            u32::from_le_bytes(
                data[AUTO_INSTALL_LEN_OFFSET..WINDOWS11_BYPASS_NRO_OFFSET]
                    .try_into()
                    .unwrap()
            ),
            0
        );
        assert_eq!(data[WINDOWS11_BYPASS_NRO_OFFSET], 0);
        assert!(data.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn builds_wimboot_jump_payload_with_ventoy_alignment() {
        let jump = b"MZjump";
        let os_param = [0x11u8; 512];
        let windows_data = [0x22u8; VENTOY_WINDOWS_DATA_HEADER_SIZE + 3];
        let original = b"MZoriginal";

        let data = build_ventoy_wimboot_jump_payload(jump, &os_param, &windows_data, original)
            .expect("payload");

        let jump_align = 16;
        let raw_len = jump_align + os_param.len() + windows_data.len() + original.len();
        assert_eq!(data.len(), 2048);
        assert!(raw_len < data.len());
        assert_eq!(&data[..jump.len()], jump);
        assert!(data[jump.len()..jump_align].iter().all(|byte| *byte == 0));
        assert_eq!(&data[jump_align..jump_align + os_param.len()], os_param);
        let windows_data_offset = jump_align + os_param.len();
        assert_eq!(
            &data[windows_data_offset..windows_data_offset + windows_data.len()],
            windows_data
        );
        let original_offset = windows_data_offset + windows_data.len();
        assert_eq!(
            &data[original_offset..original_offset + original.len()],
            original
        );
        assert!(data[raw_len..].iter().all(|byte| *byte == 0));
    }
}
