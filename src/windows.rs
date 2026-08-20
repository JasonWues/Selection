use arboard::Clipboard;
use log::{error, info};
use std::error::Error;
use std::time::{Duration, Instant};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
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

// COM has to be initialised on whichever thread calls in, and every successful
// initialisation owes a matching CoUninitialize. This used to initialise on
// every call and balance none of them, so the apartment's reference count only
// ever climbed.
//
// The flag is what makes this correct rather than merely symmetric.
// CoInitializeEx answers three different ways: S_OK for "you initialised it",
// S_FALSE for "it was already initialised and you now hold a reference" -- both
// of which we owe a CoUninitialize for -- and RPC_E_CHANGED_MODE for "somebody
// else chose a different apartment", which we owe nothing for and must not
// balance, since doing so would pull COM out from under whoever set it up.
// `HRESULT::is_ok()` is true for the first two and false for the third, which
// is exactly that distinction.
struct ComGuard(bool);

impl ComGuard {
    fn new() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

// Available for Edge, Chrome and UWP
fn get_text_by_automation() -> Result<String, Box<dyn Error>> {
    // Declared before every interface pointer below, and so dropped after all of
    // them: releasing a COM object once its apartment is gone is undefined, and
    // Rust drops locals in reverse declaration order.
    let _com = ComGuard::new();
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
    // One handle for the whole function. On Windows `Clipboard::new` is a
    // zero-sized value that opens nothing -- each operation opens and closes the
    // clipboard for itself -- so holding this across `copy()` below does not
    // keep the clipboard locked away from the application being copied from.
    let mut clipboard = Clipboard::new()?;

    // The image is only ever needed to put it back when there was no text, so it
    // is fetched only then. Reading it up front copied an entire bitmap out of
    // the clipboard and dropped it again every time the clipboard held text --
    // and text is the usual case, since something was just selected.
    let old_text = clipboard.get_text();
    let old_image = match old_text {
        Ok(_) => None,
        Err(_) => clipboard.get_image().ok(),
    };

    if !copy() {
        return Err("Copy Failed".into());
    }

    let new_text = clipboard.get_text();

    // Restore first, and unconditionally: returning early on an unreadable new
    // clipboard would leave the user's own clipboard replaced by the selection.
    //
    // Still only one format deep -- HTML, RTF and a copied file list do not
    // survive this and never did. Fixing that needs the raw clipboard API;
    // arboard knows four formats and its setters replace rather than compose.
    match (old_text, old_image) {
        (Ok(text), _) => clipboard.set_text(text)?,
        (_, Some(image)) => clipboard.set_image(image)?,
        _ => clipboard.clear()?,
    }

    Ok(new_text?.trim().to_string())
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
