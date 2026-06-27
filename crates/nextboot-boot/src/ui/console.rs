use alloc::format;
use alloc::string::String;
use core::fmt::Write;
use nextboot_menu::Input;
use uefi::prelude::*;
use uefi::proto::console::text::Output;
use uefi::ResultExt;

pub(super) fn output_text(stdout: &mut Output, text: &str) -> uefi::Result<()> {
    stdout
        .write_str(text)
        .map_err(|_| uefi::Status::DEVICE_ERROR.into())
}

pub(crate) fn show_message(st: &mut SystemTable<Boot>, msg: &str) {
    let stdout = st.stdout();
    let _ = stdout.reset(false);
    let _ = output_text(stdout, &format!("\r\n  {}\r\n", msg));
}

pub(crate) fn show_error(st: &mut SystemTable<Boot>, msg: &str) {
    let stdout = st.stdout();
    let _ = stdout.reset(false);
    let _ = output_text(stdout, &format!("\r\n  ERROR: {}\r\n", msg));
    let _ = output_text(stdout, "\r\n  Press any key to exit...\r\n");
    let _ = wait_for_key(st);
}

pub(crate) fn wait_for_key(st: &mut SystemTable<Boot>) -> Input {
    loop {
        if let Some(event) = st.stdin().wait_for_key_event() {
            let mut events = [event];
            if st
                .boot_services()
                .wait_for_event(&mut events)
                .discard_errdata()
                .is_err()
            {
                continue;
            }
            if let Some(input) = read_input_key(st) {
                return input;
            }
        }
    }
}

pub(super) fn read_password(st: &mut SystemTable<Boot>, prompt: &str) -> uefi::Result<String> {
    output_text(st.stdout(), prompt)?;

    let mut password = String::new();
    loop {
        if let Some(event) = st.stdin().wait_for_key_event() {
            let mut events = [event];
            if st
                .boot_services()
                .wait_for_event(&mut events)
                .discard_errdata()
                .is_err()
            {
                continue;
            }

            if let Ok(Some(key)) = st.stdin().read_key() {
                match key {
                    uefi::proto::console::text::Key::Printable(c) => {
                        let ch = char::from(c);
                        match ch {
                            '\r' | '\n' => {
                                output_text(st.stdout(), "\r\n")?;
                                return Ok(password);
                            }
                            '\x08' | '\x7f' => {
                                if !password.is_empty() {
                                    password.pop();
                                    output_text(st.stdout(), "\x08 \x08")?;
                                }
                            }
                            ch if ch >= ' ' => {
                                password.push(ch);
                                output_text(st.stdout(), "*")?;
                            }
                            _ => {}
                        }
                    }
                    uefi::proto::console::text::Key::Special(_) => {}
                }
            }
        }
    }
}

pub(super) fn wait_for_key_or_timeout(
    st: &mut SystemTable<Boot>,
    timeout_seconds: Option<u64>,
) -> Option<Input> {
    use uefi::table::boot::{EventType, TimerTrigger, Tpl};

    let Some(seconds) = timeout_seconds else {
        return Some(wait_for_key(st));
    };
    if seconds == 0 {
        return None;
    }

    let Some(key_event) = st.stdin().wait_for_key_event() else {
        return Some(wait_for_key(st));
    };
    let timer_event = match unsafe {
        st.boot_services()
            .create_event(EventType::TIMER, Tpl::APPLICATION, None, None)
    } {
        Ok(event) => event,
        Err(_) => return Some(wait_for_key(st)),
    };

    let timer_ticks = seconds.saturating_mul(10_000_000);
    if st
        .boot_services()
        .set_timer(&timer_event, TimerTrigger::Relative(timer_ticks))
        .is_err()
    {
        let _ = st.boot_services().close_event(timer_event);
        return Some(wait_for_key(st));
    }

    let mut events = [key_event, timer_event];
    let signaled = st
        .boot_services()
        .wait_for_event(&mut events)
        .discard_errdata()
        .ok();
    let (input, fallback_to_key_wait) = match signaled {
        Some(0) => {
            let input = read_input_key(st);
            let fallback = input.is_none();
            (input, fallback)
        }
        Some(1) => (None, false),
        _ => (None, true),
    };
    let [_, timer_event] = events;
    let _ = st.boot_services().close_event(timer_event);
    if fallback_to_key_wait {
        Some(wait_for_key(st))
    } else {
        input
    }
}

fn read_input_key(st: &mut SystemTable<Boot>) -> Option<Input> {
    let key = st.stdin().read_key().ok().flatten()?;
    match key {
        uefi::proto::console::text::Key::Special(sc) => Some(Input::from_uefi_key(sc.0, None)),
        uefi::proto::console::text::Key::Printable(c) => {
            Some(Input::from_uefi_key(0, Some(char::from(c))))
        }
    }
}
