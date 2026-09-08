//! Glue between terminal keys and the scan detector in `taskologic_core`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use taskologic_core::barcode::{ScanDetector, ScanKey};
use taskologic_core::prefs::ScannerPrefs;

pub fn detector_for(prefs: &ScannerPrefs) -> ScanDetector {
    ScanDetector::new()
        .with_prefix(&prefs.prefix)
        .with_enter_terminates(prefs.presses_enter)
}

/// Keys the detector cares about. Anything with Ctrl or Alt is never part
/// of a barcode.
pub fn scan_key(k: &KeyEvent) -> Option<ScanKey> {
    let modified = k
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
    match k.code {
        KeyCode::Char(c) if !modified => Some(ScanKey::Char(c)),
        KeyCode::Enter if !modified => Some(ScanKey::Enter),
        _ => None,
    }
}

/// A key the detector held and then released back to normal handling.
pub fn key_event(k: ScanKey) -> KeyEvent {
    match k {
        ScanKey::Char(c) => {
            let m = if c.is_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            KeyEvent::new(KeyCode::Char(c), m)
        }
        ScanKey::Enter => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    }
}

/// The barcode input injector: a payload as the sequence of terminal events
/// a wedge scanner would produce. Tests feed these to the app.
#[cfg(test)]
pub fn inject(payload: &str, press_enter: bool) -> Vec<crossterm::event::Event> {
    use crossterm::event::Event;
    let mut out: Vec<Event> = payload
        .chars()
        .map(|c| Event::Key(key_event(ScanKey::Char(c))))
        .collect();
    if press_enter {
        out.push(Event::Key(key_event(ScanKey::Enter)));
    }
    out
}
