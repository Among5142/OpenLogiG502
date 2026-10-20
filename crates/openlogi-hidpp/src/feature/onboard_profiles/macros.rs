//! Host-free decoding and encoding of the macros an `0x8100` onboard profile
//! binds a button to (macro format `0x01`).
//!
//! **Reverse-engineered.** A macro is a run of commands starting at the byte
//! address its binding names, ended by `ff`. Opcodes are libratbag's
//! (`src/hidpp20.h`, `HIDPP20_MACRO_*`); every command used here is three
//! bytes, an opcode and two payload bytes (`union hidpp20_macro_data`).
//!
//! What a G502 X LIGHTSPEED proves: a macro G HUB wrote ("tap 3, then
//! Space+X", read back from its sector 6) uses key press `43 mods usage`, key
//! release `44 mods usage` and delay `40 hi lo` in milliseconds, big-endian
//! as libratbag reads it. Commands run straight across libratbag's 16-byte
//! read chunks (it assumes they never do), and each macro ends `ff ff`.
//!
//! Mouse-button down `41 hi lo` and up `42 hi lo` carry the button mask
//! big-endian, the encoding a mouse-button binding uses (`80 01 hi lo`).
//! libratbag names the opcodes but never parses their payload; a left-click
//! macro written this way clicks on a G502 X LIGHTSPEED.

use super::layout::{KeyModifiers, MouseButtons};

/// `40 hi lo`: wait, in milliseconds.
const OP_DELAY: u8 = 0x40;
/// `41 hi lo`: press mouse buttons.
const OP_BUTTON_DOWN: u8 = 0x41;
/// `42 hi lo`: release mouse buttons.
const OP_BUTTON_UP: u8 = 0x42;
/// `43 mods usage`: press a key.
const OP_KEY_PRESS: u8 = 0x43;
/// `44 mods usage`: release a key.
const OP_KEY_RELEASE: u8 = 0x44;
/// Ends the macro.
const OP_END: u8 = 0xff;

/// Bytes in every command but the end marker.
pub const COMMAND_LEN: usize = 3;

/// Bytes a sector's trailing CRC takes.
const CRC_LEN: usize = 2;

/// One macro command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum MacroCommand {
    /// Wait this many milliseconds.
    Delay(u16),
    /// Press these mouse buttons.
    ButtonDown(MouseButtons),
    /// Release these mouse buttons.
    ButtonUp(MouseButtons),
    /// Press a key with these modifiers.
    KeyPress {
        /// Modifiers pressed with the key.
        modifiers: KeyModifiers,
        /// HID keyboard usage (page `0x07`); `0` presses the modifiers alone.
        usage: u8,
    },
    /// Release a key with these modifiers.
    KeyRelease {
        /// Modifiers released with the key.
        modifiers: KeyModifiers,
        /// HID keyboard usage (page `0x07`); `0` releases the modifiers alone.
        usage: u8,
    },
}

/// Why a macro could not be decoded or encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MacroError {
    /// An opcode this decoder does not read (a jump, a repeat, a consumer
    /// key, pointer movement…).
    #[error("macro command 0x{opcode:02x} at byte 0x{address:04x} is not decoded")]
    UnknownCommand {
        /// The opcode.
        opcode: u8,
        /// Where in the sector it sits.
        address: usize,
    },
    /// The commands run into the CRC without an end marker.
    #[error("the macro at byte 0x{0:04x} has no end marker")]
    Unterminated(usize),
}

/// The commands and end marker need more room than a sector has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the macro needs {needed} bytes but a sector holds {capacity}")]
pub struct MacroTooLong {
    /// Bytes the macro needs.
    pub needed: usize,
    /// Bytes a sector has for it.
    pub capacity: usize,
}

impl MacroCommand {
    fn encode(self) -> [u8; COMMAND_LEN] {
        let wide = |opcode: u8, value: u16| {
            let [hi, lo] = value.to_be_bytes();
            [opcode, hi, lo]
        };
        match self {
            Self::Delay(ms) => wide(OP_DELAY, ms),
            Self::ButtonDown(buttons) => wide(OP_BUTTON_DOWN, buttons.bits()),
            Self::ButtonUp(buttons) => wide(OP_BUTTON_UP, buttons.bits()),
            Self::KeyPress { modifiers, usage } => [OP_KEY_PRESS, modifiers.bits(), usage],
            Self::KeyRelease { modifiers, usage } => [OP_KEY_RELEASE, modifiers.bits(), usage],
        }
    }
}

/// Decodes the macro starting at byte `address` of a full sector, up to its
/// end marker.
pub fn decode_macro(sector: &[u8], address: usize) -> Result<Vec<MacroCommand>, MacroError> {
    let body = sector.len().saturating_sub(CRC_LEN);
    let mut commands = Vec::new();
    let mut at = address;
    loop {
        let Some(&opcode) = sector.get(at).filter(|_| at < body) else {
            return Err(MacroError::Unterminated(address));
        };
        if opcode == OP_END {
            return Ok(commands);
        }
        if at + COMMAND_LEN > body {
            return Err(MacroError::Unterminated(address));
        }
        let (a, b) = (sector[at + 1], sector[at + 2]);
        let wide = u16::from_be_bytes([a, b]);
        commands.push(match opcode {
            OP_DELAY => MacroCommand::Delay(wide),
            OP_BUTTON_DOWN => MacroCommand::ButtonDown(MouseButtons::from_bits_retain(wide)),
            OP_BUTTON_UP => MacroCommand::ButtonUp(MouseButtons::from_bits_retain(wide)),
            OP_KEY_PRESS => MacroCommand::KeyPress {
                modifiers: KeyModifiers::from_bits_retain(a),
                usage: b,
            },
            OP_KEY_RELEASE => MacroCommand::KeyRelease {
                modifiers: KeyModifiers::from_bits_retain(a),
                usage: b,
            },
            _ => {
                return Err(MacroError::UnknownCommand {
                    opcode,
                    address: at,
                });
            }
        });
        at += COMMAND_LEN;
    }
}

/// A sealed sector of `sector_len` bytes holding `commands` at byte 0: the
/// commands, `ff ff` as G HUB ends a macro, erased `ff` up to the CRC.
pub fn macro_sector(commands: &[MacroCommand], sector_len: usize) -> Result<Vec<u8>, MacroTooLong> {
    let capacity = sector_len.saturating_sub(CRC_LEN);
    let needed = commands.len() * COMMAND_LEN + 2;
    if needed > capacity {
        return Err(MacroTooLong { needed, capacity });
    }
    let mut sector = vec![0xff; sector_len];
    for (index, command) in commands.iter().enumerate() {
        let at = index * COMMAND_LEN;
        sector[at..at + COMMAND_LEN].copy_from_slice(&command.encode());
    }
    super::layout::seal(&mut sector);
    Ok(sector)
}

/// Whether a sector holds nothing: every byte erased, CRC included.
#[must_use]
pub fn sector_blank(sector: &[u8]) -> bool {
    sector.iter().all(|&byte| byte == 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::onboard_profiles::layout::sector_crc_valid;

    /// Sector 6 of a G502 X LIGHTSPEED: one G HUB macro, "tap 3, then
    /// Space+X" with recorded timing, stored four times (at 0x00, 0x26,
    /// 0x4c, 0x72) for the two layers of two buttons.
    const G502X_MACRO_SECTOR: &str = "\
        43 00 20 40 00 46 44 00 20 40 00 c8 40 00 5a 43 \
        00 2c 40 00 41 43 00 1b 40 00 4c 44 00 2c 40 00 \
        37 44 00 1b ff ff 43 00 20 40 00 46 44 00 20 40 \
        00 c8 40 00 5a 43 00 2c 40 00 41 43 00 1b 40 00 \
        4c 44 00 2c 40 00 37 44 00 1b ff ff 43 00 20 40 \
        00 46 44 00 20 40 00 c8 40 00 5a 43 00 2c 40 00 \
        41 43 00 1b 40 00 4c 44 00 2c 40 00 37 44 00 1b \
        ff ff 43 00 20 40 00 46 44 00 20 40 00 c8 40 00 \
        5a 43 00 2c 40 00 41 43 00 1b 40 00 4c 44 00 2c \
        40 00 37 44 00 1b ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff ff \
        ff ff ff ff ff ff ff ff ff ff ff ff ff 68 1f";

    fn hex(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    fn press(usage: u8) -> MacroCommand {
        MacroCommand::KeyPress {
            modifiers: KeyModifiers::empty(),
            usage,
        }
    }

    fn release(usage: u8) -> MacroCommand {
        MacroCommand::KeyRelease {
            modifiers: KeyModifiers::empty(),
            usage,
        }
    }

    fn g_hub_macro() -> Vec<MacroCommand> {
        vec![
            press(0x20),
            MacroCommand::Delay(70),
            release(0x20),
            MacroCommand::Delay(200),
            MacroCommand::Delay(90),
            press(0x2c),
            MacroCommand::Delay(65),
            press(0x1b),
            MacroCommand::Delay(76),
            release(0x2c),
            MacroCommand::Delay(55),
            release(0x1b),
        ]
    }

    #[test]
    fn a_g_hub_macro_decodes_at_each_of_its_copies() {
        let sector = hex(G502X_MACRO_SECTOR);
        assert_eq!(sector.len(), 255);
        assert!(sector_crc_valid(&sector));
        for address in [0x00, 0x26, 0x4c, 0x72] {
            assert_eq!(decode_macro(&sector, address), Ok(g_hub_macro()));
        }
    }

    #[test]
    fn a_written_macro_reads_back_and_matches_g_hub_bytes() {
        let sector = macro_sector(&g_hub_macro(), 255).expect("fits");
        assert!(sector_crc_valid(&sector));
        assert_eq!(decode_macro(&sector, 0), Ok(g_hub_macro()));
        assert_eq!(&sector[..0x26], &hex(G502X_MACRO_SECTOR)[..0x26]);
    }

    #[test]
    fn mouse_buttons_round_trip_as_a_big_endian_mask() {
        let click = [
            MacroCommand::ButtonDown(MouseButtons::LEFT),
            MacroCommand::Delay(0x1234),
            MacroCommand::ButtonUp(MouseButtons::LEFT),
        ];
        let sector = macro_sector(&click, 255).expect("fits");
        assert_eq!(
            &sector[..10],
            &[0x41, 0, 1, 0x40, 0x12, 0x34, 0x42, 0, 1, 0xff]
        );
        assert_eq!(decode_macro(&sector, 0), Ok(click.to_vec()));
    }

    #[test]
    fn what_cannot_be_read_is_refused() {
        let mut sector = vec![0xff; 255];
        sector[..3].copy_from_slice(&[0x60, 0x00, 0x07]);
        assert_eq!(
            decode_macro(&sector, 0),
            Err(MacroError::UnknownCommand {
                opcode: 0x60,
                address: 0
            })
        );
        let mut endless = vec![0x40; 255];
        endless[253..].copy_from_slice(&[0, 0]);
        assert_eq!(decode_macro(&endless, 0), Err(MacroError::Unterminated(0)));
    }

    #[test]
    fn a_macro_too_long_for_a_sector_is_refused() {
        let fits = vec![MacroCommand::Delay(1); 83];
        assert!(macro_sector(&fits, 255).is_ok_and(|sector| sector.len() == 255));
        let too_many = vec![MacroCommand::Delay(1); 84];
        assert_eq!(
            macro_sector(&too_many, 255),
            Err(MacroTooLong {
                needed: 254,
                capacity: 253
            })
        );
    }

    #[test]
    fn only_an_erased_sector_is_blank() {
        assert!(sector_blank(&[0xff; 255]));
        assert!(!sector_blank(&hex(G502X_MACRO_SECTOR)));
    }
}
