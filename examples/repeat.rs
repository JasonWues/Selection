//! Calls `get_text` several times in a row and reports what came back, plus the
//! clipboard before and after.
//!
//! The point is the repetition. `get_text_by_automation` initialises COM on the
//! calling thread and now balances it with `CoUninitialize` when it was the one
//! that initialised it; if that balance is wrong, the first call still succeeds
//! and a later one fails with the apartment gone. A single call proves nothing.
//!
//! Run it with something selected in another window to exercise the clipboard
//! path as well:
//!
//! ```text
//! cargo run --example repeat
//! ```

use std::time::Instant;

fn main() {
    let mut clipboard = arboard::Clipboard::new().expect("clipboard");
    let sentinel = "selection-example-sentinel";
    clipboard.set_text(sentinel).expect("seed the clipboard");
    println!("clipboard before: {:?}\n", clipboard.get_text().ok());

    for i in 1..=5 {
        let started = Instant::now();
        let text = selection::get_text();
        println!(
            "call {i}: {:>4}ms  {:?}",
            started.elapsed().as_millis(),
            text
        );
    }

    // Only meaningful when the clipboard path actually ran -- with nothing
    // selected the synthesised Ctrl+C changes nothing and there is nothing to
    // put back.
    println!("\nclipboard after:  {:?}", clipboard.get_text().ok());
}
