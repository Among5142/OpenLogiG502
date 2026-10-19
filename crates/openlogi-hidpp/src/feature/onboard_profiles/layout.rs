//! Host-free decoding of the `0x8100` on-flash memory layout: the CRC that
//! seals every sector, the profile directory, and the profile sectors with
//! their button bindings.
//!
//! **Reverse-engineered.** Logitech's public HID++ spec drop does not cover
//! `0x8100`. Every offset here was cross-checked between two independent
//! implementations that drive real G-series mice: libratbag
//! (`src/hidpp20.c`, `union hidpp20_internal_profile`,
//! `union hidpp20_button_binding`) and Solaar
//! (`lib/logitech_receiver/hidpp20.py`, `OnboardProfile`, `Button`). Where the
//! two disagree, or a field's meaning is inferred, the comment says so.
//!
//! The decoders here never touch a device; [`super::OnboardProfilesFeature`]
//! reads the bytes and hands them in.

use num_enum::TryFromPrimitive;

/// The sector holding the user profile directory (libratbag
/// `HIDPP20_USER_PROFILES_G402`).
pub const USER_DIRECTORY_SECTOR: u16 = 0x0000;

/// The first factory (ROM) profile sector is `ROM_SECTOR_BASE + 1`; ROM
/// profiles are numbered from 1 like user profiles (libratbag
/// `HIDPP20_ROM_PROFILES_G402`).
pub const ROM_SECTOR_BASE: u16 = 0x0100;

/// Directory entries end at the first entry whose sector reads `0xffff`.
const DIRECTORY_END: u16 = 0xffff;

/// Size of one directory entry: sector (BE u16), enabled flag, one reserved byte.
const DIRECTORY_ENTRY_LEN: usize = 4;

/// Profile sectors hold 16 primary and 16 G-shift binding slots regardless of
/// how many buttons the device has; unused slots read `ff ff ff ff`.
pub const BINDING_SLOTS: usize = 16;

/// Size of one encoded button binding.
const BINDING_LEN: usize = 4;

/// Profile-sector offsets shared by profile formats `0x01`–`0x05`.
mod offset {
    /// Report interval in milliseconds.
    pub const REPORT_INTERVAL: usize = 0x00;
    /// Index of the default DPI slot.
    pub const DEFAULT_DPI_INDEX: usize = 0x01;
    /// Index of the DPI slot the DPI-shift ("sniper") button switches to.
    pub const SHIFT_DPI_INDEX: usize = 0x02;
    /// Five little-endian u16 DPI slots.
    pub const DPI: usize = 0x03;
    /// 16 primary button bindings.
    pub const BUTTONS: usize = 0x20;
    /// 16 G-shift button bindings.
    pub const GSHIFT_BUTTONS: usize = 0x60;
    /// 48-byte profile name.
    pub const NAME: usize = 0xa0;
    /// Length of the name field.
    pub const NAME_LEN: usize = 48;
}

/// Number of DPI slots in a profile.
pub const DPI_SLOTS: usize = 5;

/// The smallest sector that holds every field [`ProfileSector::decode`]
/// reads, plus the trailing CRC.
const MIN_PROFILE_SECTOR_LEN: usize = offset::NAME + offset::NAME_LEN + 2;

/// CRC-16/CCITT-FALSE (poly `0x1021`, seed `0xffff`, no reflection, no final
/// XOR) — the checksum stored big-endian in the last two bytes of every
/// sector. Matches libratbag `hidpp_crc_ccitt` and Solaar `common.crc16`.
#[must_use]
pub fn crc_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xffff;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}

/// Whether a full sector's trailing big-endian CRC matches its contents.
///
/// A sector shorter than the CRC itself is never valid.
#[must_use]
pub fn sector_crc_valid(sector: &[u8]) -> bool {
    let Some(body_len) = sector.len().checked_sub(2) else {
        return false;
    };
    let stored = u16::from_be_bytes([sector[body_len], sector[body_len + 1]]);
    crc_ccitt(&sector[..body_len]) == stored
}

/// Why a sector could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// The sector is shorter than the fields being decoded.
    #[error("sector is {actual} bytes, decoding needs at least {needed}")]
    TooShort {
        /// Bytes needed.
        needed: usize,
        /// Bytes supplied.
        actual: usize,
    },
    /// The directory ran to the end of the sector without an end marker.
    #[error("profile directory has no 0xffff end marker")]
    UnterminatedDirectory,
}

/// One profile directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DirectoryEntry {
    /// The sector the profile lives in. User profiles normally sit at
    /// `0x0001..`, but the directory is authoritative.
    pub sector: u16,
    /// Whether the profile takes part in profile cycling.
    pub enabled: bool,
}

/// Decodes the user profile directory sector.
///
/// The CRC is not checked here; call [`sector_crc_valid`] first, since an
/// invalid directory means the device is running from its ROM profiles.
pub fn decode_directory(sector: &[u8]) -> Result<Vec<DirectoryEntry>, LayoutError> {
    // The final two bytes are the CRC, never an entry.
    let body = sector
        .len()
        .checked_sub(2)
        .map(|len| &sector[..len])
        .ok_or(LayoutError::TooShort {
            needed: 2,
            actual: sector.len(),
        })?;
    let mut entries = Vec::new();
    for entry in body.chunks_exact(DIRECTORY_ENTRY_LEN) {
        let sector = u16::from_be_bytes([entry[0], entry[1]]);
        if sector == DIRECTORY_END {
            return Ok(entries);
        }
        entries.push(DirectoryEntry {
            sector,
            // libratbag treats any non-zero byte as enabled; Solaar keeps the
            // raw byte. Firmware writes 0 or 1.
            enabled: entry[2] != 0,
        });
    }
    Err(LayoutError::UnterminatedDirectory)
}

bitflags::bitflags! {
    /// The `0x8100` mouse-button mask: bit *n* is mouse button *n + 1*, sent
    /// big-endian (libratbag decodes it with `ffs()`). Several bits may be set.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize))]
    pub struct MouseButtons: u16 {
        /// Button 1 (left).
        const LEFT = 1 << 0;
        /// Button 2 (right).
        const RIGHT = 1 << 1;
        /// Button 3 (middle).
        const MIDDLE = 1 << 2;
        /// Button 4 (back).
        const BACK = 1 << 3;
        /// Button 5 (forward).
        const FORWARD = 1 << 4;
        // Buttons 6–16 have no fixed role; `from_bits_retain` keeps them.
    }
}

bitflags::bitflags! {
    /// The HID keyboard modifier byte sent with a key binding.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize))]
    pub struct KeyModifiers: u8 {
        /// Left Control.
        const LEFT_CTRL = 1 << 0;
        /// Left Shift.
        const LEFT_SHIFT = 1 << 1;
        /// Left Alt.
        const LEFT_ALT = 1 << 2;
        /// Left GUI (Windows / Command).
        const LEFT_GUI = 1 << 3;
        /// Right Control.
        const RIGHT_CTRL = 1 << 4;
        /// Right Shift.
        const RIGHT_SHIFT = 1 << 5;
        /// Right Alt.
        const RIGHT_ALT = 1 << 6;
        /// Right GUI.
        const RIGHT_GUI = 1 << 7;
    }
}

/// Firmware-implemented button functions (behavior nibble `0x9`). Names
/// follow Solaar's `ButtonFunctions`; libratbag's `HIDPP20_BUTTON_SPECIAL_*`
/// agrees on every value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, TryFromPrimitive)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[repr(u8)]
pub enum ButtonFunction {
    /// Does nothing.
    NoAction = 0x00,
    /// Horizontal scroll left (wheel tilt).
    TiltLeft = 0x01,
    /// Horizontal scroll right (wheel tilt).
    TiltRight = 0x02,
    /// Next DPI slot.
    NextDpi = 0x03,
    /// Previous DPI slot.
    PreviousDpi = 0x04,
    /// Cycle DPI slots.
    CycleDpi = 0x05,
    /// Return to the default DPI slot.
    DefaultDpi = 0x06,
    /// Hold for the shift DPI slot ("sniper").
    ShiftDpi = 0x07,
    /// Next profile.
    NextProfile = 0x08,
    /// Previous profile.
    PreviousProfile = 0x09,
    /// Cycle profiles.
    CycleProfile = 0x0a,
    /// Hold to use the G-shift binding layer.
    GShift = 0x0b,
    /// Show battery status on the device LEDs.
    BatteryStatus = 0x0c,
    /// Select the profile named in the binding's data byte.
    ProfileSelect = 0x0d,
    /// Switch performance mode.
    ModeSwitch = 0x0e,
    /// Host-switch button.
    HostButton = 0x0f,
    /// Scroll down one notch.
    ScrollDown = 0x10,
    /// Scroll up one notch.
    ScrollUp = 0x11,
}

/// Macro behaviors (behavior nibbles `0x0`–`0x2`). Solaar names all three;
/// libratbag only parses `Execute`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, TryFromPrimitive)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[repr(u8)]
pub enum MacroBehavior {
    /// Run the macro.
    Execute = 0x0,
    /// Stop this macro.
    Stop = 0x1,
    /// Stop every running macro.
    StopAll = 0x2,
}

/// What a button slot does, decoded from its four bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ButtonBinding {
    /// `ff ff ff ff`: the slot is unassigned.
    Unassigned,
    /// `80 00 ..`: a HID binding that sends nothing.
    NoAction,
    /// `80 01 hi lo`: press these mouse buttons.
    Mouse(MouseButtons),
    /// `80 02 mods usage`: press a keyboard usage with modifiers.
    Key {
        /// Modifiers held with the key.
        modifiers: KeyModifiers,
        /// HID keyboard usage (page `0x07`).
        usage: u8,
    },
    /// `80 03 hi lo`: press a consumer-control usage (page `0x0c`).
    Consumer(u16),
    /// `90 fn .. data`: a firmware function.
    Function {
        /// The function.
        function: ButtonFunction,
        /// Function argument; the profile number for
        /// [`ButtonFunction::ProfileSelect`], otherwise 0.
        data: u8,
    },
    /// `0s ss aa aa` (behavior nibble 0–2): a macro stored on the device.
    Macro {
        /// What the press does to the macro.
        behavior: MacroBehavior,
        /// 12-bit sector holding the macro.
        sector: u16,
        /// Byte address of the macro within that sector.
        address: u16,
    },
}

/// The four bytes of a slot no decoder recognises, kept for display and so a
/// later write can put them back untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[error("unrecognised button binding {0:02x?}")]
pub struct UnknownBinding(pub [u8; BINDING_LEN]);

/// Type byte of a HID-report binding (behavior nibble `0x8`, low nibble 0;
/// libratbag `HIDPP20_BUTTON_HID_TYPE`).
const TYPE_SEND: u8 = 0x80;
/// Type byte of a firmware-function binding (behavior nibble `0x9`, low
/// nibble 0; libratbag `HIDPP20_BUTTON_SPECIAL`).
const TYPE_FUNCTION: u8 = 0x90;

impl ButtonBinding {
    /// Decodes one 4-byte slot.
    pub fn decode(bytes: [u8; BINDING_LEN]) -> Result<Self, UnknownBinding> {
        if bytes == [0xff; BINDING_LEN] {
            return Ok(Self::Unassigned);
        }
        let wide = u16::from_be_bytes([bytes[2], bytes[3]]);
        match bytes[0] {
            TYPE_SEND => match bytes[1] {
                0x00 => Ok(Self::NoAction),
                0x01 => Ok(Self::Mouse(MouseButtons::from_bits_retain(wide))),
                0x02 => Ok(Self::Key {
                    modifiers: KeyModifiers::from_bits_retain(bytes[2]),
                    usage: bytes[3],
                }),
                0x03 => Ok(Self::Consumer(wide)),
                _ => Err(UnknownBinding(bytes)),
            },
            TYPE_FUNCTION => ButtonFunction::try_from(bytes[1])
                .map(|function| Self::Function {
                    function,
                    data: bytes[3],
                })
                .map_err(|_| UnknownBinding(bytes)),
            // Macros carry a 12-bit sector in the low nibble and next byte.
            first => MacroBehavior::try_from(first >> 4)
                .map(|behavior| Self::Macro {
                    behavior,
                    sector: u16::from_be_bytes([bytes[0] & 0x0f, bytes[1]]),
                    address: wide,
                })
                .map_err(|_| UnknownBinding(bytes)),
        }
    }
}

/// A decoded profile sector. Lighting, power-save timers and the remaining
/// reserved bytes are not decoded yet; a writer must start from the raw
/// sector so they survive.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ProfileSector {
    /// Report interval in milliseconds (`1` = 1000 Hz).
    pub report_interval_ms: u8,
    /// Index into [`dpi`](Self::dpi) used by default.
    pub default_dpi_index: u8,
    /// Index into [`dpi`](Self::dpi) the DPI-shift button switches to.
    pub shift_dpi_index: u8,
    /// DPI slots; `0` marks an unused slot.
    pub dpi: [u16; DPI_SLOTS],
    /// Primary bindings, one per slot.
    pub buttons: [Result<ButtonBinding, UnknownBinding>; BINDING_SLOTS],
    /// G-shift layer bindings, one per slot.
    pub gshift_buttons: [Result<ButtonBinding, UnknownBinding>; BINDING_SLOTS],
    /// Profile name, or `None` while it is still the erased (`0xff`) default.
    /// UTF-16LE per Solaar; libratbag treats it as opaque bytes.
    pub name: Option<String>,
}

impl ProfileSector {
    /// Decodes a full profile sector. The CRC is not checked here; see
    /// [`sector_crc_valid`].
    pub fn decode(sector: &[u8]) -> Result<Self, LayoutError> {
        if sector.len() < MIN_PROFILE_SECTOR_LEN {
            return Err(LayoutError::TooShort {
                needed: MIN_PROFILE_SECTOR_LEN,
                actual: sector.len(),
            });
        }
        let slot = |base: usize, index: usize| {
            let start = base + index * BINDING_LEN;
            let mut raw = [0; BINDING_LEN];
            raw.copy_from_slice(&sector[start..start + BINDING_LEN]);
            ButtonBinding::decode(raw)
        };
        Ok(Self {
            report_interval_ms: sector[offset::REPORT_INTERVAL],
            default_dpi_index: sector[offset::DEFAULT_DPI_INDEX],
            shift_dpi_index: sector[offset::SHIFT_DPI_INDEX],
            dpi: std::array::from_fn(|i| {
                let at = offset::DPI + 2 * i;
                u16::from_le_bytes([sector[at], sector[at + 1]])
            }),
            buttons: std::array::from_fn(|i| slot(offset::BUTTONS, i)),
            gshift_buttons: std::array::from_fn(|i| slot(offset::GSHIFT_BUTTONS, i)),
            name: decode_name(&sector[offset::NAME..offset::NAME + offset::NAME_LEN]),
        })
    }
}

/// UTF-16LE, NUL-padded; an all-`0xff` field is the erased default.
fn decode_name(raw: &[u8]) -> Option<String> {
    if raw.iter().all(|&b| b == 0xff) {
        return None;
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0 && unit != 0xffff)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC-16/CCITT-FALSE check value from the CRC catalogue.
    #[test]
    fn crc_matches_the_ccitt_false_check_value() {
        assert_eq!(crc_ccitt(b"123456789"), 0x29b1);
    }

    fn seal(mut sector: Vec<u8>) -> Vec<u8> {
        let len = sector.len();
        let crc = crc_ccitt(&sector[..len - 2]);
        sector[len - 2..].copy_from_slice(&crc.to_be_bytes());
        sector
    }

    #[test]
    fn a_sealed_sector_validates_and_a_flipped_byte_does_not() {
        let mut sector = seal(vec![0xff; 255]);
        assert!(sector_crc_valid(&sector));
        sector[10] ^= 1;
        assert!(!sector_crc_valid(&sector));
        assert!(!sector_crc_valid(&[0x12]));
    }

    #[test]
    fn the_directory_stops_at_the_end_marker() {
        let mut sector = vec![0xff; 255];
        sector[..12].copy_from_slice(&[
            0x00, 0x01, 0x01, 0x00, //
            0x00, 0x02, 0x00, 0x00, //
            0xff, 0xff, 0x00, 0x00,
        ]);
        let entries = decode_directory(&seal(sector)).unwrap();
        assert_eq!(
            entries,
            [
                DirectoryEntry {
                    sector: 1,
                    enabled: true
                },
                DirectoryEntry {
                    sector: 2,
                    enabled: false
                },
            ]
        );
    }

    #[test]
    fn a_directory_without_an_end_marker_is_an_error() {
        let sector = vec![0x00; 18];
        assert_eq!(
            decode_directory(&sector),
            Err(LayoutError::UnterminatedDirectory)
        );
    }

    #[test]
    fn bindings_decode_per_behavior() {
        let decode = ButtonBinding::decode;
        assert_eq!(decode([0xff; 4]), Ok(ButtonBinding::Unassigned));
        assert_eq!(
            decode([0x80, 0x01, 0x00, 0x01]),
            Ok(ButtonBinding::Mouse(MouseButtons::LEFT))
        );
        assert_eq!(
            decode([0x80, 0x01, 0x00, 0x10]),
            Ok(ButtonBinding::Mouse(MouseButtons::FORWARD))
        );
        assert_eq!(
            decode([0x80, 0x02, 0x01, 0x06]),
            Ok(ButtonBinding::Key {
                modifiers: KeyModifiers::LEFT_CTRL,
                usage: 0x06,
            })
        );
        assert_eq!(
            decode([0x80, 0x03, 0x00, 0xe9]),
            Ok(ButtonBinding::Consumer(0x00e9))
        );
        assert_eq!(
            decode([0x90, 0x07, 0xff, 0x00]),
            Ok(ButtonBinding::Function {
                function: ButtonFunction::ShiftDpi,
                data: 0,
            })
        );
        assert_eq!(
            decode([0x01, 0x23, 0x00, 0x40]),
            Ok(ButtonBinding::Macro {
                behavior: MacroBehavior::Execute,
                sector: 0x0123,
                address: 0x0040,
            })
        );
    }

    #[test]
    fn unrecognised_bindings_keep_their_bytes() {
        for raw in [[0x80, 0x07, 0, 0], [0x90, 0x40, 0, 0], [0x50, 0, 0, 0]] {
            assert_eq!(ButtonBinding::decode(raw), Err(UnknownBinding(raw)));
        }
    }

    /// A synthetic sector shaped like a stock G502 X profile:
    /// 1 ms reports, DPI 800/1600/3200 with the 400-DPI shift slot, and the
    /// libratbag default button order (left, right, middle, back, forward,
    /// tilt left/right, DPI up/down, DPI shift, G-shift).
    fn g502x_like_profile() -> Vec<u8> {
        let mut sector = vec![0xff; 255];
        sector[0..3].copy_from_slice(&[1, 1, 4]);
        for (i, dpi) in [800u16, 1600, 3200, 0, 400].into_iter().enumerate() {
            sector[3 + 2 * i..5 + 2 * i].copy_from_slice(&dpi.to_le_bytes());
        }
        let buttons: [[u8; 4]; 11] = [
            [0x80, 0x01, 0x00, 0x01],
            [0x80, 0x01, 0x00, 0x02],
            [0x80, 0x01, 0x00, 0x04],
            [0x80, 0x01, 0x00, 0x08],
            [0x80, 0x01, 0x00, 0x10],
            [0x90, 0x01, 0xff, 0x00],
            [0x90, 0x02, 0xff, 0x00],
            [0x90, 0x03, 0xff, 0x00],
            [0x90, 0x04, 0xff, 0x00],
            [0x90, 0x07, 0xff, 0x00],
            [0x90, 0x0b, 0xff, 0x00],
        ];
        for (i, b) in buttons.iter().enumerate() {
            sector[0x20 + 4 * i..0x24 + 4 * i].copy_from_slice(b);
        }
        let name: Vec<u8> = "Profile 1"
            .encode_utf16()
            .chain(std::iter::repeat(0))
            .take(24)
            .flat_map(u16::to_le_bytes)
            .collect();
        sector[0xa0..0xd0].copy_from_slice(&name);
        seal(sector)
    }

    #[test]
    fn a_g502x_like_profile_decodes() {
        let sector = g502x_like_profile();
        assert!(sector_crc_valid(&sector));
        let profile = ProfileSector::decode(&sector).unwrap();
        assert_eq!(profile.report_interval_ms, 1);
        assert_eq!(profile.dpi, [800, 1600, 3200, 0, 400]);
        assert_eq!(profile.shift_dpi_index, 4);
        assert_eq!(profile.name.as_deref(), Some("Profile 1"));
        assert_eq!(
            profile.buttons[3],
            Ok(ButtonBinding::Mouse(MouseButtons::BACK))
        );
        assert_eq!(
            profile.buttons[10],
            Ok(ButtonBinding::Function {
                function: ButtonFunction::GShift,
                data: 0,
            })
        );
        assert_eq!(profile.buttons[11], Ok(ButtonBinding::Unassigned));
        assert!(
            profile
                .gshift_buttons
                .iter()
                .all(|b| *b == Ok(ButtonBinding::Unassigned))
        );
    }

    /// User profile sector `0x0002` read from a G502 X LIGHTSPEED
    /// (profile format `0x03`, firmware MPM30.00_B0014) with
    /// `openlogi diag profiles`. Its stored CRC was written by the firmware or
    /// G HUB, so matching it pins [`crc_ccitt`] to real hardware.
    const G502X_PROFILE_2: &str = "\
        02 02 00 20 03 b0 04 40 06 60 09 80 0c ff ff ff \
        ff 00 ff ff ff ff ff ff ff ff ff ff 3c 00 2c 01 \
        80 01 00 01 80 01 00 02 80 01 00 04 80 01 00 08 \
        90 0b 00 00 80 01 00 10 90 01 00 00 90 02 00 00 \
        90 0a 00 00 90 03 00 00 90 04 00 00 90 10 00 00 \
        90 11 00 00 ff ff ff ff ff ff ff ff ff ff ff ff \
        80 01 00 01 80 01 00 02 80 02 01 17 80 01 00 08 \
        ff ff ff ff 80 01 00 10 80 02 03 2b 80 02 01 2b \
        80 02 01 27 80 02 01 1d 80 02 01 1b 80 03 00 ea \
        80 03 00 e9 ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        0f 00 00 00 00 00 00 00 64 00 00 0f 00 00 00 00 \
        00 00 00 64 00 00 10 00 00 00 00 00 00 00 64 00 \
        00 10 00 00 00 00 00 00 00 64 00 00 00 3b 0d";

    fn hex(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn a_real_g502x_profile_sector_validates_and_decodes() {
        let sector = hex(G502X_PROFILE_2);
        assert_eq!(sector.len(), 255);
        assert!(sector_crc_valid(&sector));

        let profile = ProfileSector::decode(&sector).unwrap();
        assert_eq!(profile.report_interval_ms, 2);
        assert_eq!(profile.default_dpi_index, 2);
        assert_eq!(profile.dpi, [800, 1200, 1600, 2400, 3200]);
        assert_eq!(profile.name, None);
        assert_eq!(
            profile.buttons[4],
            Ok(ButtonBinding::Function {
                function: ButtonFunction::GShift,
                data: 0,
            })
        );
        assert_eq!(
            profile.buttons[9],
            Ok(ButtonBinding::Function {
                function: ButtonFunction::NextDpi,
                data: 0,
            })
        );
        assert_eq!(
            profile.gshift_buttons[6],
            Ok(ButtonBinding::Key {
                modifiers: KeyModifiers::LEFT_CTRL | KeyModifiers::LEFT_SHIFT,
                usage: 0x2b,
            })
        );
        assert_eq!(profile.gshift_buttons[4], Ok(ButtonBinding::Unassigned));
        // Slots past the mouse's 11 buttons still carry G HUB's defaults.
        assert_eq!(
            profile.gshift_buttons[11],
            Ok(ButtonBinding::Consumer(0x00ea))
        );
    }

    #[test]
    fn an_erased_name_is_none_and_a_short_sector_is_refused() {
        let mut sector = g502x_like_profile();
        sector[0xa0..0xd0].fill(0xff);
        assert_eq!(ProfileSector::decode(&sector).unwrap().name, None);
        assert_eq!(
            ProfileSector::decode(&sector[..64]),
            Err(LayoutError::TooShort {
                needed: MIN_PROFILE_SECTOR_LEN,
                actual: 64,
            })
        );
    }
}
