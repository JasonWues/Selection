use arboard::Clipboard;
use log::{error, info};
use std::error::Error;
use std::time::{Duration, Instant};
use windows::Win32::System::Com::{CoCreateInstance, CoInitialize, CLSCTX_ALL};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};
pub fn get_text() -> String {
    match get_text_by_automation() {
        Ok(text) => {
            if !text.is_empty() {
                return text;
            } else {
                info!("get_text_by_automation is empty");
            }
        }
        Err(err) => {
            error!("get_text_by_automation error:{}", err);
        }
    }
    info!("fallback to get_text_by_clipboard");
    match get_text_by_clipboard() {
        Ok(text) => {
            if !text.is_empty() {
                return text;
            } else {
                info!("get_text_by_clipboard is empty");
            }
        }
        Err(err) => {
            error!("get_text_by_automation error:{}", err);
        }
    }
    // Return Empty String
    String::new()
}

// Available for Edge, Chrome and UWP
fn get_text_by_automation() -> Result<String, Box<dyn Error>> {
    // Init COM
    let _ = unsafe { CoInitialize(None) };
    // Create IUIAutomation instance
    let auto: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL) }?;
    // Get Focused Element
    let el = unsafe { auto.GetFocusedElement() }?;
    // Get TextPattern
    let res: IUIAutomationTextPattern = unsafe { el.GetCurrentPatternAs(UIA_TextPatternId) }?;
    // Get TextRange Array
    let text_array = unsafe { res.GetSelection() }?;
    let length = unsafe { text_array.Length() }?;
    // Iterate TextRange Array
    let mut target = String::new();
    for i in 0..length {
        let text = unsafe { text_array.GetElement(i) }?;
        let str = unsafe { text.GetText(-1) }?;
        let str = str.to_string();
        target.push_str(&str);
    }
    Ok(target.trim().to_string())
}

// Available for almost all applications
fn get_text_by_clipboard() -> Result<String, Box<dyn Error>> {
    // Read Old Clipboard
    let old_clipboard = (Clipboard::new()?.get_text(), Clipboard::new()?.get_image());

    if copy() {
        // Read New Clipboard
        let new_text = Clipboard::new()?.get_text();

        // Create Write Clipboard
        let mut write_clipboard = Clipboard::new()?;

        match old_clipboard {
            (Ok(text), _) => {
                // Old Clipboard is Text
                write_clipboard.set_text(text)?;
                if let Ok(new) = new_text {
                    Ok(new.trim().to_string())
                } else {
                    Err("New clipboard is not Text".into())
                }
            }
            (_, Ok(image)) => {
                // Old Clipboard is Image
                write_clipboard.set_image(image)?;
                if let Ok(new) = new_text {
                    Ok(new.trim().to_string())
                } else {
                    Err("New clipboard is not Text".into())
                }
            }
            _ => {
                // Old Clipboard is Empty
                write_clipboard.clear()?;
                if let Ok(new) = new_text {
                    Ok(new.trim().to_string())
                } else {
                    Err("New clipboard is not Text".into())
                }
            }
        }
    } else {
        Err("Copy Failed".into())
    }
}

// How long to give the foreground application to answer the synthesised Ctrl+C,
// and how often to look while waiting.
//
// This used to be a flat 100ms sleep followed by a single comparison, so the
// clipboard path cost 100ms even when the application had finished copying in
// five -- and still gave up on anything slower than 100ms. Watching the sequence
// number instead returns as soon as the clipboard actually changes.
//
// The budget is deliberately longer than the sleep it replaces. Raising it only
// costs anything when nothing gets copied at all -- an empty selection, or an
// application that ignores Ctrl+C -- and in that case the caller returns an
// empty string either way.
const COPY_TIMEOUT: Duration = Duration::from_millis(250);
const COPY_POLL_INTERVAL: Duration = Duration::from_millis(5);

fn copy() -> bool {
    match try_copy() {
        Ok(copied) => copied,
        Err(err) => {
            // Synthesising input can fail for reasons that have nothing to do
            // with the selection: no desktop to send to, a locked session, a
            // window running elevated. None of them is a reason to take the
            // process down from inside a hotkey handler, which is what the
            // unwraps this replaces did.
            error!("copy error:{}", err);
            false
        }
    }
}

fn try_copy() -> Result<bool, Box<dyn Error>> {
    use enigo::{
        Direction::{Click, Press, Release},
        Enigo, Key, Keyboard, Settings,
    };
    let num_before = unsafe { GetClipboardSequenceNumber() };

    let mut enigo = Enigo::new(&Settings::default())?;
    // The hotkey that got us here is itself a chord, so the user is probably
    // still holding some of these down. Releasing them first keeps the Ctrl+C
    // below from arriving as Ctrl+Shift+C, or as nothing at all.
    for key in [
        Key::Control,
        Key::Alt,
        Key::Shift,
        Key::Space,
        Key::Meta,
        Key::Tab,
        Key::Escape,
        Key::CapsLock,
        Key::C,
    ] {
        enigo.key(key, Release)?;
    }
    enigo.key(Key::Control, Press)?;
    enigo.key(Key::C, Click)?;
    enigo.key(Key::Control, Release)?;

    // The sequence number is bumped when the clipboard is closed after a write,
    // so by the time it changes the new contents are readable.
    let deadline = Instant::now() + COPY_TIMEOUT;
    loop {
        std::thread::sleep(COPY_POLL_INTERVAL);
        if unsafe { GetClipboardSequenceNumber() } != num_before {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
    }
}
