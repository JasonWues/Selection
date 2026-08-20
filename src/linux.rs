use log::{error, info};
use std::env::var;
use std::error::Error;
use std::io::Read;
use std::time::Duration;
use wl_clipboard_rs::paste::{get_contents, ClipboardType, MimeType, Seat};
use wl_clipboard_rs::utils::is_primary_selection_supported;
use x11_clipboard::Clipboard;

enum Session {
    X11,
    Wayland,
}

/// Which display server the selection should be read from.
///
/// `XDG_SESSION_TYPE` is the direct answer, but only when something set it, and
/// plenty of ways of starting a process do not: a .desktop entry launched
/// without a session manager, a systemd user unit, a login shell over ssh --
/// and WSLg, where the whole graphical stack is present and that variable is
/// simply absent. This used to give up there and return an empty string, so
/// selection lookups silently did nothing.
///
/// `WAYLAND_DISPLAY` and `DISPLAY` are set by the compositor and the X server
/// themselves, which makes them a better answer to the same question. Wayland
/// is tested first: a Wayland session usually runs Xwayland too, so `DISPLAY`
/// is set under both and cannot tell them apart on its own.
fn session() -> Option<Session> {
    match var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => return Some(Session::Wayland),
        Ok("x11") => return Some(Session::X11),
        Ok(other) if !other.is_empty() => {
            info!("unrecognised XDG_SESSION_TYPE {other}, looking at the display variables instead");
        }
        _ => {}
    }

    if var("WAYLAND_DISPLAY").is_ok() {
        Some(Session::Wayland)
    } else if var("DISPLAY").is_ok() {
        Some(Session::X11)
    } else {
        None
    }
}

pub fn get_text() -> String {
    match session() {
        Some(Session::Wayland) => match get_text_on_wayland() {
            Ok(text) => return text,
            Err(err) => error!("{}", err),
        },
        Some(Session::X11) => match get_text_on_x11() {
            Ok(text) => return text,
            Err(err) => error!("{}", err),
        },
        None => {
            error!("no graphical session: none of XDG_SESSION_TYPE, WAYLAND_DISPLAY or DISPLAY is set");
        }
    }
    // Return Empty String
    String::new()
}

fn get_text_on_x11() -> Result<String, Box<dyn Error>> {
    let clipboard = Clipboard::new()?;
    let primary = clipboard.load(
        clipboard.getter.atoms.primary,
        clipboard.getter.atoms.utf8_string,
        clipboard.getter.atoms.property,
        Duration::from_millis(100),
    )?;
    let result = String::from_utf8_lossy(&primary)
        .trim_matches('\u{0}')
        .trim()
        .to_string();
    Ok(result)
}

fn get_text_on_wayland() -> Result<String, Box<dyn Error>> {
    // Primary selection is an optional Wayland protocol; a compositor that does
    // not implement it will never answer. Xwayland usually is there, and under
    // it the selection is reachable the X11 way, so that is the fallback.
    //
    // It used to write XDG_SESSION_TYPE and GDK_BACKEND into the process
    // environment on the way past. Nothing here reads either of them again --
    // the next line already calls the X11 path directly -- so the writes bought
    // nothing, while `set_var` is not thread safe (unsafe outright from Rust
    // 2024), this runs on whichever thread the caller's hotkey landed on, and
    // pot manipulates the same environment block for its proxy settings.
    if !is_primary_selection_supported().unwrap_or(false) {
        info!("primary selection is not supported, falling back to the X11 clipboard");
        return get_text_on_x11();
    }

    let (mut pipe, _) = get_contents(ClipboardType::Primary, Seat::Unspecified, MimeType::Text)?;
    let mut contents = vec![];
    pipe.read_to_end(&mut contents)?;
    let contents = String::from_utf8_lossy(&contents)
        .trim_matches('\u{0}')
        .trim()
        .to_string();
    Ok(contents)
}
