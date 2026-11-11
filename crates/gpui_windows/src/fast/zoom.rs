//! Zooming a window that is already zoomed restores it.
//!
//! `Window::zoom_window` toggles the zoomed state on macOS and Linux. On
//! Windows it only ever maximized, so a window could not be restored through
//! it.

use windows::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, SW_MAXIMIZE, SW_RESTORE};

pub(crate) fn command(zoomed: bool) -> SHOW_WINDOW_CMD {
    if zoomed { SW_RESTORE } else { SW_MAXIMIZE }
}

#[cfg(test)]
mod tests {
    use super::{SW_MAXIMIZE, SW_RESTORE, command};

    #[test]
    fn a_zoomed_window_is_restored_and_any_other_is_maximized() {
        assert_eq!(command(true), SW_RESTORE);
        assert_eq!(command(false), SW_MAXIMIZE);
    }
}
