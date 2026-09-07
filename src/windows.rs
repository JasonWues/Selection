use arboard::Clipboard;
use log::{error, info};
use std::error::Error;
use std::time::{Duration, Instant};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
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

/// Replace the current selection with `text`, by way of the clipboard and a
/// synthesised Ctrl+V.
///
/// There is no UI Automation path for this to prefer, the way `get_text` has
/// one. `IUIAutomationTextPattern` is read-only -- it can tell you where the
/// selection is but not write through it -- and `ValuePattern`, which can
/// write, addresses a whole control rather than a range, so using it would
/// replace the entire field rather than what the user selected. The keystroke
/// is what every application already understands.
pub fn set_text(text: &str) -> bool {
    match try_set_text(text) {
        Ok(pasted) => pasted,
        Err(err) => {
            error!("set_text error:{}", err);
            false
        }
    }
}

fn try_set_text(text: &str) -> Result<bool, Box<dyn Error>> {
    // Same bargain as the copy path: the clipboard is borrowed for the length
    // of a paste and has to be given back the way it was found.
    let snapshot = Snapshot::take().unwrap_or_else(|err| {
        error!("could not snapshot the clipboard: {}", err);
        Snapshot::empty()
    });

    Clipboard::new()?.set_text(text.to_owned())?;

    let pasted = paste();

    // Nothing tells us when the target application has finished reading the
    // clipboard. It reads on its own schedule after the keystroke is delivered,
    // and reading does not bump the sequence number, so there is no event to
    // wait on -- restoring too eagerly would put the user's old clipboard back
    // before the paste had taken our text, and the user would get their
    // previous clipboard pasted instead of the translation.
    //
    // So: a settle delay, then `restore`, whose `OpenClipboard` retries for
    // another 100ms. An application still holding the clipboard open to read is
    // exactly what that retry waits out, which makes the pair rather more
    // robust than the bare sleep it looks like.
    std::thread::sleep(PASTE_SETTLE);
    if let Err(err) = snapshot.restore() {
        error!("could not restore the clipboard: {}", err);
    }

    Ok(pasted)
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
    // Every format the clipboard is holding, not just the one arboard would
    // hand back. This borrows the user's clipboard for the length of a copy,
    // so what goes back has to be what was there -- see `Snapshot`.
    //
    // A clipboard that cannot be read is not a reason to refuse to translate:
    // the selection is still worth having, and an empty snapshot restores as
    // an empty clipboard, which is what the old code did for that case too.
    let snapshot = Snapshot::take().unwrap_or_else(|err| {
        error!("could not snapshot the clipboard: {}", err);
        Snapshot::empty()
    });

    if !copy() {
        // Nothing was written, so the clipboard still holds the user's own
        // data -- putting the snapshot back would be a no-op at best. Leave it.
        return Err("Copy Failed".into());
    }

    // `Clipboard::new` on Windows is a zero-sized value that opens nothing;
    // each operation opens and closes the clipboard for itself. Constructed
    // after `copy()` so it cannot be holding anything while the foreground
    // application writes.
    let new_text = Clipboard::new()?.get_text();

    // Restored unconditionally, and before the text is unwrapped: returning
    // early on an unreadable clipboard would leave the user's own clipboard
    // replaced by the selection.
    if let Err(err) = snapshot.restore() {
        error!("could not restore the clipboard: {}", err);
    }

    Ok(new_text?.trim().to_string())
}

// The formats that are not memory blocks, and so cannot be copied by reading
// their bytes. CF_BITMAP, CF_PALETTE and the two metafile kinds are GDI
// handles; CF_OWNERDISPLAY has no data at all and the CF_DSP* pair are the
// owner-drawn equivalents of the same. A clipboard manager either duplicates
// each of these through its own API or leaves it behind, and leaving it behind
// costs nothing here: an image on the clipboard is almost always accompanied by
// CF_DIB, which *is* a memory block and does survive.
const NON_GLOBAL_FORMATS: [u32; 8] = [
    2,     // CF_BITMAP
    3,     // CF_METAFILEPICT
    9,     // CF_PALETTE
    14,    // CF_ENHMETAFILE
    0x80,  // CF_OWNERDISPLAY
    0x82,  // CF_DSPBITMAP
    0x83,  // CF_DSPMETAFILEPICT
    0x8E,  // CF_DSPENHMETAFILE
];

// Another process can hold the clipboard open, and briefly does whenever one is
// copying. Worth a few retries rather than losing the user's clipboard over a
// collision measured in milliseconds.
const CLIPBOARD_OPEN_ATTEMPTS: u32 = 10;
const CLIPBOARD_OPEN_RETRY: Duration = Duration::from_millis(10);

/// Holds the clipboard open, and closes it however the scope is left. Every
/// `OpenClipboard` owes a `CloseClipboard`, and leaving it open would lock the
/// clipboard away from every other process on the desktop.
struct ClipboardHandle;

impl ClipboardHandle {
    fn open() -> Result<Self, Box<dyn Error>> {
        for attempt in 0..CLIPBOARD_OPEN_ATTEMPTS {
            // `None` for the owner window: this process has no window to give,
            // and a null owner is what makes the clipboard task-owned rather
            // than tied to a window that may outlive the call.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Self);
            }
            if attempt + 1 < CLIPBOARD_OPEN_ATTEMPTS {
                std::thread::sleep(CLIPBOARD_OPEN_RETRY);
            }
        }
        Err("could not open the clipboard".into())
    }
}

impl Drop for ClipboardHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Everything the clipboard held, as raw bytes per format.
///
/// This is the whole point of going below arboard. arboard knows four formats
/// and its setters replace the clipboard rather than compose onto it, so
/// restoring through it collapsed whatever was there to a single flavour: HTML
/// copied from a browser came back as plain text, RTF from a word processor
/// lost its formatting, and a copied file list vanished outright -- because
/// CF_HDROP is not one of the four. Reading the formats as opaque blocks keeps
/// all of them, including the private formats applications use to paste back
/// into themselves losslessly.
struct Snapshot {
    entries: Vec<(u32, Vec<u8>)>,
}

impl Snapshot {
    fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn take() -> Result<Self, Box<dyn Error>> {
        let _handle = ClipboardHandle::open()?;

        let mut entries = Vec::new();
        let mut format = 0u32;
        loop {
            // Enumeration is a cursor: each call returns the format after the
            // one given, and 0 ends it.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            if NON_GLOBAL_FORMATS.contains(&format) {
                continue;
            }
            // The handle belongs to the clipboard and must not be freed. It can
            // also legitimately fail: a format offered through delayed
            // rendering whose owner has since died has no data behind it.
            let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
                continue;
            };
            let block = HGLOBAL(handle.0);
            // Backstop for the list above: a handle that is not in the global
            // heap sizes as 0, which is also how an empty block reads, and
            // there is nothing to copy either way.
            let size = unsafe { GlobalSize(block) };
            if size == 0 {
                continue;
            }
            let ptr = unsafe { GlobalLock(block) };
            if ptr.is_null() {
                continue;
            }
            let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec();
            unsafe {
                let _ = GlobalUnlock(block);
            }
            entries.push((format, bytes));
        }
        Ok(Self { entries })
    }

    fn restore(&self) -> Result<(), Box<dyn Error>> {
        let _handle = ClipboardHandle::open()?;

        // Takes ownership away from whoever wrote the selection, which is what
        // makes the SetClipboardData calls below legal. An empty snapshot stops
        // here, leaving an empty clipboard rather than the copied selection.
        unsafe { EmptyClipboard() }?;

        for (format, bytes) in &self.entries {
            // GMEM_MOVEABLE is required: the clipboard takes ownership of the
            // block and frees it itself, and only moveable blocks may be given
            // to it.
            let Ok(block) = (unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }) else {
                continue;
            };
            let ptr = unsafe { GlobalLock(block) };
            if ptr.is_null() {
                unsafe {
                    let _ = GlobalFree(Some(block));
                }
                continue;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                let _ = GlobalUnlock(block);
            }
            // On success the clipboard owns the block; on failure it does not,
            // and this is the only chance to give it back.
            if unsafe { SetClipboardData(*format, Some(HANDLE(block.0))) }.is_err() {
                unsafe {
                    let _ = GlobalFree(Some(block));
                }
            }
        }
        Ok(())
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

// How long to leave the translation on the clipboard before putting the user's
// own contents back. See the note in `try_set_text` for why this cannot be an
// event instead.
const PASTE_SETTLE: Duration = Duration::from_millis(200);

fn paste() -> bool {
    match send_chord(enigo::Key::V) {
        Ok(()) => true,
        Err(err) => {
            error!("paste error:{}", err);
            false
        }
    }
}

/// Ctrl + `key`, with whatever the user is still holding released first.
///
/// The hotkey that got us here is itself a chord, so those keys are probably
/// still down. Releasing them keeps the chord below from arriving as
/// Ctrl+Shift+V, or as nothing at all. `key` is released too, in case the
/// hotkey happened to contain it.
fn send_chord(key: enigo::Key) -> Result<(), Box<dyn Error>> {
    use enigo::{
        Direction::{Click, Press, Release},
        Enigo, Key, Keyboard, Settings,
    };

    let mut enigo = Enigo::new(&Settings::default())?;
    for held in [
        Key::Control,
        Key::Alt,
        Key::Shift,
        Key::Space,
        Key::Meta,
        Key::Tab,
        Key::Escape,
        Key::CapsLock,
        key,
    ] {
        enigo.key(held, Release)?;
    }
    enigo.key(Key::Control, Press)?;
    enigo.key(key, Click)?;
    enigo.key(Key::Control, Release)?;
    Ok(())
}

fn try_copy() -> Result<bool, Box<dyn Error>> {
    let num_before = unsafe { GetClipboardSequenceNumber() };

    send_chord(enigo::Key::C)?;

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
