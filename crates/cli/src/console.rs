//! Windows console handling for the GUI.
//!
//! `forge3d.exe` is a console-subsystem program so that `forge3d mcp` keeps
//! its stdio. When the GUI is started from Explorer or a shortcut, Windows
//! creates a console just for it; [`detach_if_owned`] closes that window. A GUI
//! started from a terminal shares the console with the shell and keeps it.

/// Free the console when this process is its only user (double-click start).
/// Returns whether the console was detached. Always false off Windows.
#[cfg(windows)]
pub fn detach_if_owned() -> bool {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut pids = [0u32; 4];
    // SAFETY: `pids` is a valid buffer of the length passed alongside it.
    let users = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    if users == 1 {
        // SAFETY: no arguments; only detaches this process from its console.
        unsafe { FreeConsole() != 0 }
    } else {
        false
    }
}

#[cfg(not(windows))]
pub fn detach_if_owned() -> bool {
    false
}

/// Show a fatal error to the user: on stderr, or in a message box when the
/// console was detached and nobody would see stderr.
pub fn report_fatal(message: &str, detached: bool) {
    eprintln!("error: {message}");
    #[cfg(windows)]
    if detached {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        let (text, title) = (wide(message), wide("forge3d"));
        // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the call.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = detached;
}
