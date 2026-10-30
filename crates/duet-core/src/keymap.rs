//! W3C `KeyboardEvent.code` → enigo keys.
//!
//! Guests send physical key codes (`"KeyA"`, `"ArrowLeft"`) so the host types
//! the same key regardless of either side's keyboard layout quirks.

use duet_protocol::Modifiers;
use enigo::Key;

pub fn key_for_code(code: &str) -> Option<Key> {
    if let Some(c) = code.strip_prefix("Key") {
        let ch = c.chars().next()?;
        return (c.len() == 1 && ch.is_ascii_alphabetic()).then(|| Key::Unicode(ch.to_ascii_lowercase()));
    }
    if let Some(d) = code.strip_prefix("Digit") {
        let ch = d.chars().next()?;
        return (d.len() == 1 && ch.is_ascii_digit()).then_some(Key::Unicode(ch));
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return function_key(n);
    }
    Some(match code {
        "Enter" | "NumpadEnter" => Key::Return,
        "Tab" => Key::Tab,
        "Space" => Key::Space,
        "Backspace" => Key::Backspace,
        "Escape" => Key::Escape,
        "Delete" => Key::Delete,
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "CapsLock" => Key::CapsLock,
        "ShiftLeft" | "ShiftRight" => Key::Shift,
        "ControlLeft" | "ControlRight" => Key::Control,
        "AltLeft" | "AltRight" => Key::Alt,
        "MetaLeft" | "MetaRight" => Key::Meta,
        "Minus" => Key::Unicode('-'),
        "Equal" => Key::Unicode('='),
        "BracketLeft" => Key::Unicode('['),
        "BracketRight" => Key::Unicode(']'),
        "Backslash" => Key::Unicode('\\'),
        "Semicolon" => Key::Unicode(';'),
        "Quote" => Key::Unicode('\''),
        "Backquote" => Key::Unicode('`'),
        "Comma" => Key::Unicode(','),
        "Period" => Key::Unicode('.'),
        "Slash" => Key::Unicode('/'),
        _ => return None,
    })
}

fn function_key(n: u8) -> Option<Key> {
    Some(match n {
        1 => Key::F1,
        2 => Key::F2,
        3 => Key::F3,
        4 => Key::F4,
        5 => Key::F5,
        6 => Key::F6,
        7 => Key::F7,
        8 => Key::F8,
        9 => Key::F9,
        10 => Key::F10,
        11 => Key::F11,
        12 => Key::F12,
        _ => return None,
    })
}

/// Modifier keys to hold while a key is pressed, in press order.
///
/// `command` is the "shortcut" modifier: ⌘ on a Mac host, Ctrl on Windows and
/// Linux, so Cmd+C from a Mac guest copies on a Windows host too.
pub fn modifier_keys(m: &Modifiers) -> Vec<Key> {
    let mut keys = Vec::new();
    if m.control {
        keys.push(Key::Control);
    }
    if m.option {
        keys.push(Key::Alt);
    }
    if m.shift {
        keys.push(Key::Shift);
    }
    if m.command {
        keys.push(shortcut_key());
    }
    keys.dedup();
    keys
}

pub fn shortcut_key() -> Key {
    if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    }
}

/// Is this code itself a modifier key? Those are sent via [`Modifiers`]
/// instead, so the host never gets stuck with a held modifier.
pub fn is_modifier_code(code: &str) -> bool {
    matches!(
        code,
        "ShiftLeft" | "ShiftRight" | "ControlLeft" | "ControlRight" | "AltLeft" | "AltRight"
            | "MetaLeft" | "MetaRight"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_named_keys() {
        assert_eq!(key_for_code("KeyQ"), Some(Key::Unicode('q')));
        assert_eq!(key_for_code("Digit7"), Some(Key::Unicode('7')));
        assert_eq!(key_for_code("Enter"), Some(Key::Return));
        assert_eq!(key_for_code("F12"), Some(Key::F12));
        assert_eq!(key_for_code("Slash"), Some(Key::Unicode('/')));
        assert_eq!(key_for_code("F13"), None);
        assert_eq!(key_for_code("KeyAB"), None);
        assert_eq!(key_for_code("Unidentified"), None);
    }

    #[test]
    fn command_maps_to_platform_shortcut() {
        let m = Modifiers { command: true, shift: true, ..Default::default() };
        assert_eq!(modifier_keys(&m), vec![Key::Shift, shortcut_key()]);
        // Ctrl + Cmd on Windows collapse to a single Ctrl.
        let both = Modifiers { command: true, control: true, ..Default::default() };
        assert_eq!(modifier_keys(&both).len(), if cfg!(target_os = "macos") { 2 } else { 1 });
    }
}
