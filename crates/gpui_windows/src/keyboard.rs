use anyhow::Result;
use collections::HashMap;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        GetKeyboardLayoutNameW, MAPVK_VK_TO_CHAR, MAPVK_VK_TO_VSC, MapVirtualKeyW, ToUnicode,
        VIRTUAL_KEY, VK_0, VK_1, VK_2, VK_3, VK_4, VK_5, VK_6, VK_7, VK_8, VK_9, VK_ABNT_C1,
        VK_CONTROL, VK_MENU, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7,
        VK_OEM_8, VK_OEM_102, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_SHIFT,
    },
    WindowsAndMessaging::KL_NAMELENGTH,
};

use gpui::{
    KeybindingKeystroke, Keystroke, Modifiers, PlatformKeyboardLayout, PlatformKeyboardMapper,
};

pub(crate) struct WindowsKeyboardLayout {
    id: String,
    name: String,
}

pub(crate) struct WindowsKeyboardMapper {
    key_to_vkey: HashMap<String, (u16, bool)>,
    vkey_to_key: HashMap<u16, String>,
    vkey_to_shifted: HashMap<u16, String>,
}

impl PlatformKeyboardLayout for WindowsKeyboardLayout {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl PlatformKeyboardMapper for WindowsKeyboardMapper {
    fn map_key_equivalent(
        &self,
        mut keystroke: Keystroke,
        use_key_equivalents: bool,
    ) -> KeybindingKeystroke {
        let Some((vkey, shifted_key)) = self.get_vkey_from_key(&keystroke.key, use_key_equivalents)
        else {
            return KeybindingKeystroke::from_keystroke(keystroke);
        };
        if shifted_key && keystroke.modifiers.shift {
            log::warn!(
                "Keystroke '{}' has both shift and a shifted key, this is likely a bug",
                keystroke.key
            );
        }

        let shift = shifted_key || keystroke.modifiers.shift;
        keystroke.modifiers.shift = false;

        let Some(key) = self.vkey_to_key.get(&vkey).cloned() else {
            log::error!(
                "Failed to map key equivalent '{:?}' to a valid key",
                keystroke
            );
            return KeybindingKeystroke::from_keystroke(keystroke);
        };

        keystroke.key = if shift {
            let Some(shifted_key) = self.vkey_to_shifted.get(&vkey).cloned() else {
                log::error!(
                    "Failed to map keystroke {:?} with virtual key '{:?}' to a shifted key",
                    keystroke,
                    vkey
                );
                return KeybindingKeystroke::from_keystroke(keystroke);
            };
            shifted_key
        } else {
            key.clone()
        };

        let modifiers = Modifiers {
            shift,
            ..keystroke.modifiers
        };

        KeybindingKeystroke::new(keystroke, modifiers, key)
    }

    fn get_key_equivalents(&self) -> Option<&HashMap<char, char>> {
        None
    }
}

impl WindowsKeyboardLayout {
    pub(crate) fn new() -> Result<Self> {
        let mut buffer = [0u16; KL_NAMELENGTH as usize]; // KL_NAMELENGTH includes the null terminator
        unsafe { GetKeyboardLayoutNameW(&mut buffer)? };
        let id = String::from_utf16_lossy(&buffer[..buffer.len() - 1]); // Remove the null terminator
        let entry = windows_registry::LOCAL_MACHINE.open(format!(
            "System\\CurrentControlSet\\Control\\Keyboard Layouts\\{id}"
        ))?;
        let name = entry.get_string("Layout Text")?;
        Ok(Self { id, name })
    }

    pub(crate) fn unknown() -> Self {
        Self {
            id: "unknown".to_string(),
            name: "unknown".to_string(),
        }
    }
}

impl WindowsKeyboardMapper {
    pub(crate) fn new() -> Self {
        let mut key_to_vkey = HashMap::default();
        let mut vkey_to_key = HashMap::default();
        let mut vkey_to_shifted = HashMap::default();
        for vkey in CANDIDATE_VKEYS {
            if let Some(key) = get_key_from_vkey(*vkey) {
                key_to_vkey.insert(key.clone(), (vkey.0, false));
                vkey_to_key.insert(vkey.0, key);
            }
            let scan_code = unsafe { MapVirtualKeyW(vkey.0 as u32, MAPVK_VK_TO_VSC) };
            if scan_code == 0 {
                continue;
            }
            if let Some(shifted_key) = get_shifted_key(*vkey, scan_code) {
                key_to_vkey.insert(shifted_key.clone(), (vkey.0, true));
                vkey_to_shifted.insert(vkey.0, shifted_key);
            }
        }
        Self {
            key_to_vkey,
            vkey_to_key,
            vkey_to_shifted,
        }
    }

    fn get_vkey_from_key(&self, key: &str, use_key_equivalents: bool) -> Option<(u16, bool)> {
        if use_key_equivalents {
            get_vkey_from_key_with_us_layout(key)
        } else {
            self.key_to_vkey.get(key).cloned()
        }
    }
}

pub(crate) fn get_keystroke_key(
    vkey: VIRTUAL_KEY,
    scan_code: u32,
    modifiers: &mut Modifiers,
) -> Option<String> {
    if modifiers.shift && need_to_convert_to_shifted_key(vkey) {
        get_shifted_key(vkey, scan_code).inspect(|_| {
            modifiers.shift = false;
        })
    } else {
        get_key_from_vkey(vkey)
    }
}

fn get_key_from_vkey(vkey: VIRTUAL_KEY) -> Option<String> {
    let key_data = unsafe { MapVirtualKeyW(vkey.0 as u32, MAPVK_VK_TO_CHAR) };
    if key_data == 0 {
        return None;
    }

    // The high word contains dead key flag, the low word contains the character
    let key = char::from_u32(key_data & 0xFFFF)?;

    Some(key.to_ascii_lowercase().to_string())
}

#[inline]
fn need_to_convert_to_shifted_key(vkey: VIRTUAL_KEY) -> bool {
    matches!(
        vkey,
        VK_OEM_3
            | VK_OEM_MINUS
            | VK_OEM_PLUS
            | VK_OEM_4
            | VK_OEM_5
            | VK_OEM_6
            | VK_OEM_1
            | VK_OEM_7
            | VK_OEM_COMMA
            | VK_OEM_PERIOD
            | VK_OEM_2
            | VK_OEM_102
            | VK_OEM_8
            | VK_ABNT_C1
            | VK_0
            | VK_1
            | VK_2
            | VK_3
            | VK_4
            | VK_5
            | VK_6
            | VK_7
            | VK_8
            | VK_9
    )
}

fn get_shifted_key(vkey: VIRTUAL_KEY, scan_code: u32) -> Option<String> {
    generate_key_char(vkey, scan_code, false, true, false)
}

pub(crate) fn generate_key_char(
    vkey: VIRTUAL_KEY,
    scan_code: u32,
    control: bool,
    shift: bool,
    alt: bool,
) -> Option<String> {
    let mut state = [0; 256];
    if control {
        state[VK_CONTROL.0 as usize] = 0x80;
    }
    if shift {
        state[VK_SHIFT.0 as usize] = 0x80;
    }
    if alt {
        state[VK_MENU.0 as usize] = 0x80;
    }

    let mut buffer = [0; 8];
    let len = unsafe { ToUnicode(vkey.0 as u32, scan_code, Some(&state), &mut buffer, 0x5) };

    match len {
        len if len > 0 => String::from_utf16(&buffer[..len as usize])
            .ok()
            .filter(|candidate| {
                !candidate.is_empty() && !candidate.chars().next().unwrap().is_control()
            }),
        len if len < 0 => String::from_utf16(&buffer[..(-len as usize)]).ok(),
        _ => None,
    }
}

fn get_vkey_from_key_with_us_layout(key: &str) -> Option<(u16, bool)> {
    match key {
        // ` => VK_OEM_3
        "`" => Some((VK_OEM_3.0, false)),
        "~" => Some((VK_OEM_3.0, true)),
        "1" => Some((VK_1.0, false)),
        "!" => Some((VK_1.0, true)),
        "2" => Some((VK_2.0, false)),
        "@" => Some((VK_2.0, true)),
        "3" => Some((VK_3.0, false)),
        "#" => Some((VK_3.0, true)),
        "4" => Some((VK_4.0, false)),
        "$" => Some((VK_4.0, true)),
        "5" => Some((VK_5.0, false)),
        "%" => Some((VK_5.0, true)),
        "6" => Some((VK_6.0, false)),
        "^" => Some((VK_6.0, true)),
        "7" => Some((VK_7.0, false)),
        "&" => Some((VK_7.0, true)),
        "8" => Some((VK_8.0, false)),
        "*" => Some((VK_8.0, true)),
        "9" => Some((VK_9.0, false)),
        "(" => Some((VK_9.0, true)),
        "0" => Some((VK_0.0, false)),
        ")" => Some((VK_0.0, true)),
        "-" => Some((VK_OEM_MINUS.0, false)),
        "_" => Some((VK_OEM_MINUS.0, true)),
        "=" => Some((VK_OEM_PLUS.0, false)),
        "+" => Some((VK_OEM_PLUS.0, true)),
        "[" => Some((VK_OEM_4.0, false)),
        "{" => Some((VK_OEM_4.0, true)),
        "]" => Some((VK_OEM_6.0, false)),
        "}" => Some((VK_OEM_6.0, true)),
        "\\" => Some((VK_OEM_5.0, false)),
        "|" => Some((VK_OEM_5.0, true)),
        ";" => Some((VK_OEM_1.0, false)),
        ":" => Some((VK_OEM_1.0, true)),
        "'" => Some((VK_OEM_7.0, false)),
        "\"" => Some((VK_OEM_7.0, true)),
        "," => Some((VK_OEM_COMMA.0, false)),
        "<" => Some((VK_OEM_COMMA.0, true)),
        "." => Some((VK_OEM_PERIOD.0, false)),
        ">" => Some((VK_OEM_PERIOD.0, true)),
        "/" => Some((VK_OEM_2.0, false)),
        "?" => Some((VK_OEM_2.0, true)),
        _ => None,
    }
}

const CANDIDATE_VKEYS: &[VIRTUAL_KEY] = &[
    VK_OEM_3,
    VK_OEM_MINUS,
    VK_OEM_PLUS,
    VK_OEM_4,
    VK_OEM_5,
    VK_OEM_6,
    VK_OEM_1,
    VK_OEM_7,
    VK_OEM_COMMA,
    VK_OEM_PERIOD,
    VK_OEM_2,
    VK_OEM_102,
    VK_OEM_8,
    VK_ABNT_C1,
    VK_0,
    VK_1,
    VK_2,
    VK_3,
    VK_4,
    VK_5,
    VK_6,
    VK_7,
    VK_8,
    VK_9,
];

/// Names the physical key of a Windows set-1 scan code like the DOM `KeyboardEvent.code`,
/// following Chromium's `dom_code_data.inc`. `extended` is the `KF_EXTENDED` (0xE0 prefix) flag.
pub(crate) fn dom_code(scan_code: u16, extended: bool) -> Option<&'static str> {
    if extended {
        return Some(match scan_code {
            0x1C => "NumpadEnter",
            0x1D => "ControlRight",
            0x35 => "NumpadDivide",
            0x37 => "PrintScreen",
            0x38 => "AltRight",
            0x45 => "NumLock",
            0x47 => "Home",
            0x48 => "ArrowUp",
            0x49 => "PageUp",
            0x4B => "ArrowLeft",
            0x4D => "ArrowRight",
            0x4F => "End",
            0x50 => "ArrowDown",
            0x51 => "PageDown",
            0x52 => "Insert",
            0x53 => "Delete",
            0x5B => "MetaLeft",
            0x5C => "MetaRight",
            0x5D => "ContextMenu",
            _ => return None,
        });
    }
    Some(match scan_code {
        0x01 => "Escape",
        0x02 => "Digit1",
        0x03 => "Digit2",
        0x04 => "Digit3",
        0x05 => "Digit4",
        0x06 => "Digit5",
        0x07 => "Digit6",
        0x08 => "Digit7",
        0x09 => "Digit8",
        0x0A => "Digit9",
        0x0B => "Digit0",
        0x0C => "Minus",
        0x0D => "Equal",
        0x0E => "Backspace",
        0x0F => "Tab",
        0x10 => "KeyQ",
        0x11 => "KeyW",
        0x12 => "KeyE",
        0x13 => "KeyR",
        0x14 => "KeyT",
        0x15 => "KeyY",
        0x16 => "KeyU",
        0x17 => "KeyI",
        0x18 => "KeyO",
        0x19 => "KeyP",
        0x1A => "BracketLeft",
        0x1B => "BracketRight",
        0x1C => "Enter",
        0x1D => "ControlLeft",
        0x1E => "KeyA",
        0x1F => "KeyS",
        0x20 => "KeyD",
        0x21 => "KeyF",
        0x22 => "KeyG",
        0x23 => "KeyH",
        0x24 => "KeyJ",
        0x25 => "KeyK",
        0x26 => "KeyL",
        0x27 => "Semicolon",
        0x28 => "Quote",
        0x29 => "Backquote",
        0x2A => "ShiftLeft",
        0x2B => "Backslash",
        0x2C => "KeyZ",
        0x2D => "KeyX",
        0x2E => "KeyC",
        0x2F => "KeyV",
        0x30 => "KeyB",
        0x31 => "KeyN",
        0x32 => "KeyM",
        0x33 => "Comma",
        0x34 => "Period",
        0x35 => "Slash",
        0x36 => "ShiftRight",
        0x37 => "NumpadMultiply",
        0x38 => "AltLeft",
        0x39 => "Space",
        0x3A => "CapsLock",
        0x3B => "F1",
        0x3C => "F2",
        0x3D => "F3",
        0x3E => "F4",
        0x3F => "F5",
        0x40 => "F6",
        0x41 => "F7",
        0x42 => "F8",
        0x43 => "F9",
        0x44 => "F10",
        0x45 => "Pause",
        0x46 => "ScrollLock",
        0x47 => "Numpad7",
        0x48 => "Numpad8",
        0x49 => "Numpad9",
        0x4A => "NumpadSubtract",
        0x4B => "Numpad4",
        0x4C => "Numpad5",
        0x4D => "Numpad6",
        0x4E => "NumpadAdd",
        0x4F => "Numpad1",
        0x50 => "Numpad2",
        0x51 => "Numpad3",
        0x52 => "Numpad0",
        0x53 => "NumpadDecimal",
        0x56 => "IntlBackslash",
        0x57 => "F11",
        0x58 => "F12",
        0x59 => "NumpadEqual",
        0x64 => "F13",
        0x65 => "F14",
        0x66 => "F15",
        0x67 => "F16",
        0x68 => "F17",
        0x69 => "F18",
        0x6A => "F19",
        0x6B => "F20",
        0x6C => "F21",
        0x6D => "F22",
        0x6E => "F23",
        0x70 => "KanaMode",
        0x73 => "IntlRo",
        0x76 => "F24",
        0x79 => "Convert",
        0x7B => "NonConvert",
        0x7D => "IntlYen",
        0x7E => "NumpadComma",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn scan_codes_map_to_dom_codes() {
        use super::dom_code;
        assert_eq!(dom_code(0x1E, false), Some("KeyA"));
        assert_eq!(dom_code(0x52, false), Some("Numpad0"));
        assert_eq!(dom_code(0x0B, false), Some("Digit0"));
        assert_eq!(dom_code(0x52, true), Some("Insert"));
        assert_eq!(dom_code(0x2A, false), Some("ShiftLeft"));
        assert_eq!(dom_code(0x36, false), Some("ShiftRight"));
        assert_eq!(dom_code(0x1D, false), Some("ControlLeft"));
        assert_eq!(dom_code(0x1D, true), Some("ControlRight"));
        assert_eq!(dom_code(0x38, true), Some("AltRight"));
        assert_eq!(dom_code(0x1C, true), Some("NumpadEnter"));
        assert_eq!(dom_code(0x45, true), Some("NumLock"));
        assert_eq!(dom_code(0x45, false), Some("Pause"));
        assert_eq!(dom_code(0x5B, true), Some("MetaLeft"));
        assert_eq!(dom_code(0x00, false), None);
    }

    use crate::WindowsKeyboardMapper;
    use gpui::{Keystroke, Modifiers, PlatformKeyboardMapper};

    #[test]
    fn test_keyboard_mapper() {
        let mapper = WindowsKeyboardMapper::new();

        // Normal case
        let keystroke = Keystroke {
            modifiers: Modifiers::control(),
            key: "a".to_string(),
            key_char: None,
        };
        let mapped = mapper.map_key_equivalent(keystroke.clone(), true);
        assert_eq!(*mapped.inner(), keystroke);
        assert_eq!(mapped.key(), "a");
        assert_eq!(*mapped.modifiers(), Modifiers::control());

        // Shifted case, ctrl-$
        let keystroke = Keystroke {
            modifiers: Modifiers::control(),
            key: "$".to_string(),
            key_char: None,
        };
        let mapped = mapper.map_key_equivalent(keystroke.clone(), true);
        assert_eq!(*mapped.inner(), keystroke);
        assert_eq!(mapped.key(), "4");
        assert_eq!(*mapped.modifiers(), Modifiers::control_shift());

        // Shifted case, but shift is true
        let keystroke = Keystroke {
            modifiers: Modifiers::control_shift(),
            key: "$".to_string(),
            key_char: None,
        };
        let mapped = mapper.map_key_equivalent(keystroke, true);
        assert_eq!(mapped.inner().modifiers, Modifiers::control());
        assert_eq!(mapped.key(), "4");
        assert_eq!(*mapped.modifiers(), Modifiers::control_shift());

        // Windows style
        let keystroke = Keystroke {
            modifiers: Modifiers::control_shift(),
            key: "4".to_string(),
            key_char: None,
        };
        let mapped = mapper.map_key_equivalent(keystroke, true);
        assert_eq!(mapped.inner().modifiers, Modifiers::control());
        assert_eq!(mapped.inner().key, "$");
        assert_eq!(mapped.key(), "4");
        assert_eq!(*mapped.modifiers(), Modifiers::control_shift());
    }
}
