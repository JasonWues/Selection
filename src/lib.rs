// Project: selection
// File: lib.rs
// Created Date: 2023-06-04
// Author: Pylogmon <pylogmon@outlook.com>

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use crate::linux::get_text as _get_text;
#[cfg(target_os = "macos")]
use crate::macos::get_text as _get_text;
#[cfg(target_os = "windows")]
use crate::windows::get_text as _get_text;

/// Get the text selected by the cursor
///
/// Return empty string if no text is selected or error occurred
/// # Example
///
/// ```
/// use selection::get_text;
///
/// let text = get_text();
/// println!("{}", text);
/// ```
pub fn get_text() -> String {
    _get_text().trim().to_owned()
}

/// Replace the text selected by the cursor with `text`
///
/// Returns whether the replacement was sent. Implemented on Windows only; the
/// other platforms answer `false` rather than pretending. Both need a path that
/// does not exist here yet -- macOS wants the accessibility permission this
/// crate's `get_text` already asks for and a synthesised Cmd+V behind it, and on
/// Linux the answer depends on the session, since Wayland has no sanctioned way
/// for one application to synthesise input into another.
///
/// # Example
///
/// ```no_run
/// use selection::{get_text, set_text};
///
/// let text = get_text();
/// if !text.is_empty() {
///     set_text(&text.to_uppercase());
/// }
/// ```
pub fn set_text(text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        crate::windows::set_text(text)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = text;
        log::warn!("selection::set_text is not implemented on this platform");
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::get_text;
    #[test]
    fn it_works() {
        println!("{}", get_text());
    }
}
