use super::console::{output_text, read_password, wait_for_key};
use crate::scanner;
use alloc::format;
use uefi::prelude::*;

pub(super) fn authorize_boot_password(
    st: &mut SystemTable<Boot>,
    iso_files: &[scanner::IsoFile],
) -> uefi::Result<bool> {
    let Some(password) = iso_files
        .iter()
        .find_map(|iso| iso.ventoy_boot_password.as_ref())
    else {
        return Ok(true);
    };

    for attempt in 0..3 {
        output_text(st.stdout(), "\r\n  Boot menu password required\r\n")?;
        let input = read_password(st, "  Enter password: ")?;
        if password.verify(&input) {
            output_text(st.stdout(), "\r\n")?;
            return Ok(true);
        }

        if attempt < 2 {
            output_text(st.stdout(), "\r\n  Invalid password.\r\n")?;
        }
    }

    output_text(
        st.stdout(),
        "\r\n  Invalid password. Press any key to exit.",
    )?;
    let _ = wait_for_key(st);
    Ok(false)
}

pub(super) fn authorize_iso(
    st: &mut SystemTable<Boot>,
    iso: &scanner::IsoFile,
) -> uefi::Result<bool> {
    let Some(password) = iso.ventoy_password.as_ref() else {
        return Ok(true);
    };

    output_text(
        st.stdout(),
        &format!("\r\n  Password required for {}\r\n", iso.path),
    )?;
    let input = read_password(st, "  Enter password: ")?;
    if password.verify(&input) {
        output_text(st.stdout(), "\r\n")?;
        Ok(true)
    } else {
        output_text(
            st.stdout(),
            "\r\n  Invalid password. Press any key to return to menu.",
        )?;
        let _ = wait_for_key(st);
        Ok(false)
    }
}
