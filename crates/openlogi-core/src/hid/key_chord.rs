//! Raw USB HID keyboard chords — the modifier byte and keyboard-page usage a
//! device stores and replays itself, such as a G-series mouse's onboard
//! binding. Pure data, no I/O.
//!
//! Unlike [`crate::binding::KeyCombo`], which OpenLogi injects through each
//! OS and so limits to keys every backend can type, a [`HidKeyChord`] is
//! sent by the device as a HID report: any key on the keyboard page can be
//! stored, a modifier on its own included, and no modifier is translated
//! per platform.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The HID keyboard modifier byte (keyboard page usages `0xE0`–`0xE7`).
/// Every bit names a modifier, so every byte is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct HidModifiers(u8);

impl HidModifiers {
    /// No modifier.
    pub const NONE: Self = Self(0);
    /// Left Control.
    pub const LEFT_CTRL: Self = Self(1 << 0);
    /// Left Shift.
    pub const LEFT_SHIFT: Self = Self(1 << 1);
    /// Left Alt (Option).
    pub const LEFT_ALT: Self = Self(1 << 2);
    /// Left GUI (Windows / Command / Super).
    pub const LEFT_GUI: Self = Self(1 << 3);
    /// Right Control.
    pub const RIGHT_CTRL: Self = Self(1 << 4);
    /// Right Shift.
    pub const RIGHT_SHIFT: Self = Self(1 << 5);
    /// Right Alt (AltGr).
    pub const RIGHT_ALT: Self = Self(1 << 6);
    /// Right GUI.
    pub const RIGHT_GUI: Self = Self(1 << 7);

    /// Each modifier with its display name, in label order.
    const NAMED: [(Self, &'static str); 8] = [
        (Self::LEFT_CTRL, "Ctrl"),
        (Self::LEFT_SHIFT, "Shift"),
        (Self::LEFT_ALT, "Alt"),
        (Self::LEFT_GUI, "Win"),
        (Self::RIGHT_CTRL, "Right Ctrl"),
        (Self::RIGHT_SHIFT, "Right Shift"),
        (Self::RIGHT_ALT, "Right Alt"),
        (Self::RIGHT_GUI, "Right Win"),
    ];

    /// The modifier set the byte encodes.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// The byte as the device stores it.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether no modifier is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every modifier in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// `self` with `other`'s modifiers added.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// `self` with `other`'s modifiers removed.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The modifiers set, each with its display name, in label order.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        Self::NAMED
            .into_iter()
            .filter(move |(flag, _)| self.contains(*flag))
            .map(|(_, name)| name)
    }

    /// The single modifier `name` names, accepting the usual aliases
    /// (case-insensitive).
    fn parse_one(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "left ctrl" | "lctrl" => Self::LEFT_CTRL,
            "shift" | "left shift" | "lshift" => Self::LEFT_SHIFT,
            "alt" | "option" | "left alt" | "lalt" => Self::LEFT_ALT,
            "win" | "gui" | "super" | "meta" | "cmd" | "command" | "left win" | "lwin" => {
                Self::LEFT_GUI
            }
            "right ctrl" | "rctrl" => Self::RIGHT_CTRL,
            "right shift" | "rshift" => Self::RIGHT_SHIFT,
            "right alt" | "ralt" | "altgr" => Self::RIGHT_ALT,
            "right win" | "rwin" | "rgui" => Self::RIGHT_GUI,
            _ => return None,
        })
    }
}

/// A key on the HID keyboard page (usage page `0x07`) that has a name in
/// the USB HID Usage Tables: letters, digits, punctuation, editing and
/// navigation keys, F1–F24, the keypad, system keys, international and
/// language keys, and the modifier keys themselves (`0xE0`–`0xE7`), which
/// bind a modifier on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct HidKey(u8);

/// Why a byte is not a [`HidKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("0x{0:02x} is not a named HID keyboard usage")]
pub struct UnknownHidKey(pub u8);

impl TryFrom<u8> for HidKey {
    type Error = UnknownHidKey;

    fn try_from(usage: u8) -> Result<Self, Self::Error> {
        if key_name(usage).is_some() {
            Ok(Self(usage))
        } else {
            Err(UnknownHidKey(usage))
        }
    }
}

impl From<HidKey> for u8 {
    fn from(key: HidKey) -> Self {
        key.0
    }
}

impl HidKey {
    /// The keyboard-page usage.
    #[must_use]
    pub const fn usage(self) -> u8 {
        self.0
    }

    /// The key's display name, e.g. `"A"`, `"Page Up"`, `"Keypad 7"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        // Construction guarantees the usage has a name.
        key_name(self.0).unwrap_or("?")
    }

    /// Every named key, in usage order.
    pub fn all() -> impl Iterator<Item = Self> {
        KEY_NAMES.iter().map(|&(usage, _)| Self(usage))
    }

    /// The keys of a standard US 104-key keyboard, plus F13–F24, in usage
    /// order — what a key picker offers. F13–F24 are on no common keyboard
    /// but every OS accepts them, which makes them the usual spare keys to
    /// bind a mouse button to and catch in another program. The rest of
    /// [`Self::all`] (Non-US #, the International and Language keys, Power,
    /// the media-era Execute/Help/Menu row) reach the computer but no US
    /// keyboard has them, so they are named when read back, never offered.
    pub fn standard() -> impl Iterator<Item = Self> {
        Self::all().filter(|key| {
            matches!(
                key.0,
                0x04..=0x31 | 0x33..=0x63 | 0x65 | 0x68..=0x73 | 0xe0..=0xe7
            )
        })
    }

    /// The key a display name names (case-insensitive, also accepting a few
    /// common aliases such as `Esc`, `PgUp` and `Del`).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let wanted = name.trim();
        if let Some(&(usage, _)) = KEY_NAMES
            .iter()
            .find(|(_, known)| known.eq_ignore_ascii_case(wanted))
        {
            return Some(Self(usage));
        }
        let usage = match wanted.to_ascii_lowercase().as_str() {
            "esc" => 0x29,
            "return" => 0x28,
            "del" => 0x4c,
            "ins" => 0x49,
            "pgup" => 0x4b,
            "pgdn" | "pgdown" => 0x4e,
            "prtsc" | "print" | "printscreen" => 0x46,
            "capslock" => 0x39,
            "left" => 0x50,
            "right" => 0x4f,
            "up" => 0x52,
            "down" => 0x51,
            "space" | "spacebar" => 0x2c,
            "menu" | "app" | "apps" | "context menu" => 0x65,
            _ => return None,
        };
        Some(Self(usage))
    }

    /// Whether the key is itself a modifier (`0xE0`–`0xE7`).
    #[must_use]
    pub const fn is_modifier(self) -> bool {
        self.0 >= 0xe0
    }
}

/// A keyboard chord a device stores: modifiers held around one key. A
/// modifier on its own is the modifier's own key with no other modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HidKeyChord {
    /// Modifiers held while the key is pressed.
    pub modifiers: HidModifiers,
    /// The key pressed.
    pub key: HidKey,
}

impl HidKeyChord {
    /// `key` with no modifiers.
    #[must_use]
    pub const fn key(key: HidKey) -> Self {
        Self {
            modifiers: HidModifiers::NONE,
            key,
        }
    }
}

impl fmt::Display for HidKeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for name in self.modifiers.names() {
            write!(f, "{name}+")?;
        }
        f.write_str(self.key.name())
    }
}

/// Why typed text is not a [`HidKeyChord`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HidKeyChordParseError {
    /// Nothing was typed.
    #[error("enter a key")]
    Empty,
    /// The last part is not a key name.
    #[error("{0:?} is not a key")]
    UnknownKey(String),
    /// A part before the key is not a modifier.
    #[error("{0:?} is not a modifier")]
    UnknownModifier(String),
}

impl FromStr for HidKeyChord {
    type Err = HidKeyChordParseError;

    /// Parses `Ctrl+Shift+Insert`, `Keypad 7` or `Left Shift`: modifiers
    /// joined by `+`, then one key name. A modifier name on its own is not a
    /// key here; name its key (`Left Shift`). The keyboard page's only plus
    /// key is on the keypad, so a trailing `+` (`+`, `Ctrl++`) means
    /// `Keypad +`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() {
            return Err(HidKeyChordParseError::Empty);
        }
        // A chord ending in "+" means the plus key itself.
        let (head, key_part) = match text.strip_suffix("++") {
            Some(head) => (Some(head), "Keypad +"),
            None if text == "+" => (None, "Keypad +"),
            None => match text.rsplit_once('+') {
                Some((head, key)) => (Some(head), key),
                None => (None, text),
            },
        };
        let key = HidKey::from_name(key_part)
            .ok_or_else(|| HidKeyChordParseError::UnknownKey(key_part.trim().to_owned()))?;
        let mut modifiers = HidModifiers::NONE;
        for part in head.into_iter().flat_map(|head| head.split('+')) {
            let modifier = HidModifiers::parse_one(part.trim())
                .ok_or_else(|| HidKeyChordParseError::UnknownModifier(part.trim().to_owned()))?;
            modifiers = modifiers.with(modifier);
        }
        Ok(Self { modifiers, key })
    }
}

fn key_name(usage: u8) -> Option<&'static str> {
    KEY_NAMES
        .binary_search_by_key(&usage, |&(known, _)| known)
        .ok()
        .map(|index| KEY_NAMES[index].1)
}

/// Every named keyboard-page usage, sorted by usage (USB HID Usage Tables
/// 1.5, §10). Reserved and "error" usages are left out.
const KEY_NAMES: &[(u8, &str)] = &[
    (0x04, "A"),
    (0x05, "B"),
    (0x06, "C"),
    (0x07, "D"),
    (0x08, "E"),
    (0x09, "F"),
    (0x0a, "G"),
    (0x0b, "H"),
    (0x0c, "I"),
    (0x0d, "J"),
    (0x0e, "K"),
    (0x0f, "L"),
    (0x10, "M"),
    (0x11, "N"),
    (0x12, "O"),
    (0x13, "P"),
    (0x14, "Q"),
    (0x15, "R"),
    (0x16, "S"),
    (0x17, "T"),
    (0x18, "U"),
    (0x19, "V"),
    (0x1a, "W"),
    (0x1b, "X"),
    (0x1c, "Y"),
    (0x1d, "Z"),
    (0x1e, "1"),
    (0x1f, "2"),
    (0x20, "3"),
    (0x21, "4"),
    (0x22, "5"),
    (0x23, "6"),
    (0x24, "7"),
    (0x25, "8"),
    (0x26, "9"),
    (0x27, "0"),
    (0x28, "Enter"),
    (0x29, "Escape"),
    (0x2a, "Backspace"),
    (0x2b, "Tab"),
    (0x2c, "Space"),
    (0x2d, "-"),
    (0x2e, "="),
    (0x2f, "["),
    (0x30, "]"),
    (0x31, "\\"),
    (0x32, "Non-US #"),
    (0x33, ";"),
    (0x34, "'"),
    (0x35, "`"),
    (0x36, ","),
    (0x37, "."),
    (0x38, "/"),
    (0x39, "Caps Lock"),
    (0x3a, "F1"),
    (0x3b, "F2"),
    (0x3c, "F3"),
    (0x3d, "F4"),
    (0x3e, "F5"),
    (0x3f, "F6"),
    (0x40, "F7"),
    (0x41, "F8"),
    (0x42, "F9"),
    (0x43, "F10"),
    (0x44, "F11"),
    (0x45, "F12"),
    (0x46, "Print Screen"),
    (0x47, "Scroll Lock"),
    (0x48, "Pause"),
    (0x49, "Insert"),
    (0x4a, "Home"),
    (0x4b, "Page Up"),
    (0x4c, "Delete"),
    (0x4d, "End"),
    (0x4e, "Page Down"),
    (0x4f, "Right Arrow"),
    (0x50, "Left Arrow"),
    (0x51, "Down Arrow"),
    (0x52, "Up Arrow"),
    (0x53, "Num Lock"),
    (0x54, "Keypad /"),
    (0x55, "Keypad *"),
    (0x56, "Keypad -"),
    (0x57, "Keypad +"),
    (0x58, "Keypad Enter"),
    (0x59, "Keypad 1"),
    (0x5a, "Keypad 2"),
    (0x5b, "Keypad 3"),
    (0x5c, "Keypad 4"),
    (0x5d, "Keypad 5"),
    (0x5e, "Keypad 6"),
    (0x5f, "Keypad 7"),
    (0x60, "Keypad 8"),
    (0x61, "Keypad 9"),
    (0x62, "Keypad 0"),
    (0x63, "Keypad ."),
    (0x64, "Non-US \\"),
    (0x65, "Application"),
    (0x66, "Power"),
    (0x67, "Keypad ="),
    (0x68, "F13"),
    (0x69, "F14"),
    (0x6a, "F15"),
    (0x6b, "F16"),
    (0x6c, "F17"),
    (0x6d, "F18"),
    (0x6e, "F19"),
    (0x6f, "F20"),
    (0x70, "F21"),
    (0x71, "F22"),
    (0x72, "F23"),
    (0x73, "F24"),
    (0x74, "Execute"),
    (0x75, "Help"),
    (0x76, "Menu"),
    (0x77, "Select"),
    (0x78, "Stop"),
    (0x79, "Again"),
    (0x7a, "Undo"),
    (0x7b, "Cut"),
    (0x7c, "Copy"),
    (0x7d, "Paste"),
    (0x7e, "Find"),
    (0x7f, "Mute"),
    (0x80, "Volume Up"),
    (0x81, "Volume Down"),
    (0x85, "Keypad ,"),
    (0x87, "International 1"),
    (0x88, "International 2"),
    (0x89, "International 3"),
    (0x8a, "International 4"),
    (0x8b, "International 5"),
    (0x8c, "International 6"),
    (0x8d, "International 7"),
    (0x8e, "International 8"),
    (0x8f, "International 9"),
    (0x90, "Lang 1"),
    (0x91, "Lang 2"),
    (0x92, "Lang 3"),
    (0x93, "Lang 4"),
    (0x94, "Lang 5"),
    (0x95, "Lang 6"),
    (0x96, "Lang 7"),
    (0x97, "Lang 8"),
    (0x98, "Lang 9"),
    (0x99, "Alternate Erase"),
    (0x9a, "SysReq"),
    (0x9b, "Cancel"),
    (0x9c, "Clear"),
    (0x9d, "Prior"),
    (0x9e, "Return"),
    (0x9f, "Separator"),
    (0xa0, "Out"),
    (0xa1, "Oper"),
    (0xa2, "Clear/Again"),
    (0xa3, "CrSel"),
    (0xa4, "ExSel"),
    (0xe0, "Left Ctrl"),
    (0xe1, "Left Shift"),
    (0xe2, "Left Alt"),
    (0xe3, "Left Win"),
    (0xe4, "Right Ctrl"),
    (0xe5, "Right Shift"),
    (0xe6, "Right Alt"),
    (0xe7, "Right Win"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_table_is_sorted_and_unique() {
        assert!(
            KEY_NAMES.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "binary search needs sorted, unique usages"
        );
        let mut names: Vec<_> = KEY_NAMES
            .iter()
            .map(|(_, name)| name.to_ascii_lowercase())
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), KEY_NAMES.len(), "names must be unique");
    }

    #[test]
    fn only_named_usages_are_keys() {
        assert_eq!(HidKey::try_from(0x04).map(HidKey::name), Ok("A"));
        assert_eq!(HidKey::try_from(0x49).map(HidKey::name), Ok("Insert"));
        assert_eq!(HidKey::try_from(0xe1).map(HidKey::name), Ok("Left Shift"));
        for unnamed in [0x00, 0x01, 0x03, 0x82, 0xa5, 0xdf, 0xe8, 0xff] {
            assert_eq!(HidKey::try_from(unnamed), Err(UnknownHidKey(unnamed)));
        }
        assert_eq!(HidKey::all().count(), KEY_NAMES.len());
    }

    #[test]
    fn chords_parse_and_display() {
        let chord: HidKeyChord = "ctrl+shift+Insert".parse().unwrap();
        assert_eq!(
            chord.modifiers,
            HidModifiers::LEFT_CTRL.with(HidModifiers::LEFT_SHIFT)
        );
        assert_eq!(chord.key.usage(), 0x49);
        assert_eq!(chord.to_string(), "Ctrl+Shift+Insert");

        for (text, usage, label) in [
            ("Keypad 7", 0x5f, "Keypad 7"),
            ("PgUp", 0x4b, "Page Up"),
            ("Esc", 0x29, "Escape"),
            ("F24", 0x73, "F24"),
            ("Left Shift", 0xe1, "Left Shift"),
            ("+", 0x57, "Keypad +"),
        ] {
            let chord: HidKeyChord = text.parse().unwrap();
            assert_eq!(
                (chord.key.usage(), chord.to_string().as_str()),
                (usage, label)
            );
            assert!(chord.modifiers.is_empty());
        }
        let ctrl_plus: HidKeyChord = "Ctrl++".parse().unwrap();
        assert_eq!(ctrl_plus.to_string(), "Ctrl+Keypad +");
        let altgr: HidKeyChord = "AltGr+E".parse().unwrap();
        assert_eq!(altgr.to_string(), "Right Alt+E");
    }

    #[test]
    fn malformed_chords_say_what_is_wrong() {
        assert_eq!("".parse::<HidKeyChord>(), Err(HidKeyChordParseError::Empty));
        assert_eq!(
            "Ctrl+Warp".parse::<HidKeyChord>(),
            Err(HidKeyChordParseError::UnknownKey("Warp".into()))
        );
        assert_eq!(
            "Hyper+A".parse::<HidKeyChord>(),
            Err(HidKeyChordParseError::UnknownModifier("Hyper".into()))
        );
    }

    #[test]
    fn a_key_rejects_an_unnamed_byte_on_the_wire() {
        use serde::de::IntoDeserializer as _;
        use serde::de::value::{Error, U8Deserializer};
        let named: U8Deserializer<Error> = 0x04_u8.into_deserializer();
        assert_eq!(HidKey::deserialize(named), Ok(HidKey(0x04)));
        let unnamed: U8Deserializer<Error> = 0x82_u8.into_deserializer();
        HidKey::deserialize(unnamed).expect_err("an unnamed usage is refused");
    }

    #[test]
    fn the_standard_keys_are_a_104_key_board_and_f13_to_f24() {
        let names: Vec<&str> = HidKey::standard().map(HidKey::name).collect();
        // Each key of a US 104-key board has one usage, modifiers included.
        assert_eq!(names.len(), 104 + 12);
        for kept in [
            "A",
            "0",
            "Print Screen",
            "Keypad Enter",
            "Application",
            "F13",
            "F24",
            "Right Win",
        ] {
            assert!(names.contains(&kept), "{kept} offered");
        }
        for dropped in [
            "Non-US #",
            "Non-US \\",
            "International 1",
            "Power",
            "Keypad =",
        ] {
            assert!(!names.contains(&dropped), "{dropped} not offered");
        }
    }
}
