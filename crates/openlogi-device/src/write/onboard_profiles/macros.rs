//! Macros on `0x8100` mice: the app's [`OnboardMacro`] read from the sector
//! a binding points at, and written to a sector of its own.
//!
//! A new macro never overwrites one in use. It goes into a sector no profile
//! binds — preferably an erased one, otherwise one that holds only a macro
//! nothing points at any more (what a rebind away from a macro leaves
//! behind) — and the button is bound to it only once it reads back intact.
//! Profile sectors, the directory, and any sector holding data that is not
//! a macro are never touched.

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

use hidpp::feature::onboard_profiles::OnboardProfilesFeature;
use hidpp::feature::onboard_profiles::layout::{
    self, BINDING_SLOTS, ButtonBinding, KeyModifiers, MacroBehavior, ProfileSector,
    USER_DIRECTORY_SECTOR,
};
use hidpp::feature::onboard_profiles::macros::{self, MacroCommand, MacroTooLong};
use hidpp::protocol::v20::Hidpp20Error;
use openlogi_core::hid::{HidKey, HidKeyChord, HidModifiers, MacroEvent, MacroInput, OnboardMacro};

use super::buttons::{device_mouse_button, mouse_button};
use super::{FEATURE, OnboardWriteError, writable_active_sector, write_verified};
use crate::write::{HidppOperation, classify_hidpp_error};

/// The macro a binding runs, read from the mouse: `Some` when the binding
/// runs a macro whose every command the app reads, `None` otherwise (a stop
/// binding, a jump, a media key…). `sectors` caches sectors already read.
pub(super) async fn read_bound_macro(
    feature: &OnboardProfilesFeature,
    binding: ButtonBinding,
    sector_size: u16,
    sectors: &mut HashMap<u16, Option<Vec<u8>>>,
) -> Option<OnboardMacro> {
    let ButtonBinding::Macro {
        behavior: MacroBehavior::Execute,
        sector,
        address,
    } = binding
    else {
        return None;
    };
    if let Entry::Vacant(slot) = sectors.entry(sector) {
        slot.insert(feature.read_sector(sector, sector_size).await.ok());
    }
    let raw = sectors.get(&sector)?.as_ref()?;
    let commands = macros::decode_macro(raw, usize::from(address)).ok()?;
    let events = commands
        .into_iter()
        .map(event_from_device)
        .collect::<Option<Vec<_>>>()?;
    Some(OnboardMacro::from_events(&events))
}

/// Writes `program` to a sector no profile uses and answers with that
/// sector, to bind at address 0. Refused before anything is written when
/// the macro does not fit a sector or no sector is free.
pub(super) async fn store_macro(
    feature: &OnboardProfilesFeature,
    program: &OnboardMacro,
) -> Result<u16, OnboardWriteError> {
    let (info, _) = writable_active_sector(feature).await?;
    let commands: Vec<MacroCommand> = program
        .events()
        .iter()
        .map(|e| event_to_device(*e))
        .collect();
    let data = macros::macro_sector(&commands, usize::from(info.sector_size)).map_err(
        |MacroTooLong { needed, capacity }| OnboardWriteError::MacroTooLong {
            needed: u16::try_from(needed).unwrap_or(u16::MAX),
            capacity: u16::try_from(capacity).unwrap_or(u16::MAX),
        },
    )?;

    let fail =
        |e: Hidpp20Error| classify_hidpp_error(e, HidppOperation::WriteOnboardButton, FEATURE);
    let directory = feature
        .read_sector(USER_DIRECTORY_SECTOR, info.sector_size)
        .await
        .map_err(fail)?;
    let profiles: Vec<u16> = layout::decode_directory(&directory)
        .map_err(|_| OnboardWriteError::DirectoryInvalid)?
        .into_iter()
        .map(|entry| entry.sector)
        .collect();
    // Every macro sector any user profile binds, in either layer. A profile
    // that cannot be read could bind any sector, so nothing is written.
    let mut in_use = BTreeSet::new();
    for &profile in &profiles {
        let raw = feature
            .read_sector(profile, info.sector_size)
            .await
            .map_err(fail)?;
        let decoded = ProfileSector::decode(&raw)
            .map_err(|_| OnboardWriteError::ActiveProfileCorrupt(profile))?;
        for binding in decoded
            .buttons
            .iter()
            .chain(decoded.gshift_buttons.iter())
            .take(2 * BINDING_SLOTS)
        {
            if let Ok(ButtonBinding::Macro { sector, .. }) = binding {
                in_use.insert(*sector);
            }
        }
    }

    // Sectors past the profiles that nothing lists or binds. An erased one
    // is taken first; failing that, one holding only an unbound macro.
    let first_free = u16::from(info.profile_count) + 1;
    let mut reclaimable = None;
    for sector in first_free..u16::from(info.sector_count) {
        if sector == USER_DIRECTORY_SECTOR || profiles.contains(&sector) || in_use.contains(&sector)
        {
            continue;
        }
        let before = feature
            .read_sector(sector, info.sector_size)
            .await
            .map_err(fail)?;
        if macros::sector_blank(&before) {
            write_verified(feature, sector, before, data, info.sector_size).await?;
            return Ok(sector);
        }
        if reclaimable.is_none()
            && layout::sector_crc_valid(&before)
            && macros::decode_macro(&before, 0).is_ok()
        {
            reclaimable = Some((sector, before));
        }
    }
    let (sector, before) = reclaimable.ok_or(OnboardWriteError::MacroNoRoom)?;
    write_verified(feature, sector, before, data, info.sector_size).await?;
    Ok(sector)
}

/// The app's view of one macro command, or `None` for one it cannot show
/// (a chord of mouse buttons, a key usage it has no name for).
fn event_from_device(command: MacroCommand) -> Option<MacroEvent> {
    Some(match command {
        MacroCommand::Delay(ms) => MacroEvent::Delay(ms),
        MacroCommand::ButtonDown(buttons) => {
            MacroEvent::Press(MacroInput::Mouse(mouse_button(buttons)?))
        }
        MacroCommand::ButtonUp(buttons) => {
            MacroEvent::Release(MacroInput::Mouse(mouse_button(buttons)?))
        }
        MacroCommand::KeyPress { modifiers, usage } => {
            MacroEvent::Press(MacroInput::Key(chord(modifiers, usage)?))
        }
        MacroCommand::KeyRelease { modifiers, usage } => {
            MacroEvent::Release(MacroInput::Key(chord(modifiers, usage)?))
        }
    })
}

/// A key command's chord. Usage `0` with one modifier is that modifier
/// pressed alone, which the app names by the modifier's own key.
fn chord(modifiers: KeyModifiers, usage: u8) -> Option<HidKeyChord> {
    let modifiers = HidModifiers::from_bits(modifiers.bits());
    if usage == 0 {
        let bits = modifiers.bits();
        if bits.count_ones() != 1 {
            return None;
        }
        // Modifier bit n is keyboard usage 0xE0 + n.
        let key = HidKey::try_from(0xe0 + u8::try_from(bits.trailing_zeros()).ok()?).ok()?;
        return Some(HidKeyChord::key(key));
    }
    Some(HidKeyChord {
        modifiers,
        key: HidKey::try_from(usage).ok()?,
    })
}

pub(super) fn event_to_device(event: MacroEvent) -> MacroCommand {
    let key = |chord: HidKeyChord| {
        (
            KeyModifiers::from_bits_retain(chord.modifiers.bits()),
            chord.key.usage(),
        )
    };
    match event {
        MacroEvent::Delay(ms) => MacroCommand::Delay(ms),
        MacroEvent::Press(MacroInput::Mouse(button)) => {
            MacroCommand::ButtonDown(device_mouse_button(button))
        }
        MacroEvent::Release(MacroInput::Mouse(button)) => {
            MacroCommand::ButtonUp(device_mouse_button(button))
        }
        MacroEvent::Press(MacroInput::Key(chord)) => {
            let (modifiers, usage) = key(chord);
            MacroCommand::KeyPress { modifiers, usage }
        }
        MacroEvent::Release(MacroInput::Key(chord)) => {
            let (modifiers, usage) = key(chord);
            MacroCommand::KeyRelease { modifiers, usage }
        }
    }
}

#[cfg(test)]
mod tests {
    use hidpp::feature::onboard_profiles::layout::MouseButtons;
    use openlogi_core::hid::{MacroStep, OnboardMouseButton};

    use super::*;

    #[test]
    fn each_input_round_trips_through_its_command() {
        let ctrl_c = MacroInput::Key(HidKeyChord {
            modifiers: HidModifiers::LEFT_CTRL,
            key: HidKey::try_from(0x06).unwrap(),
        });
        let shift = MacroInput::Key(HidKeyChord::key(HidKey::try_from(0xe1).unwrap()));
        let click = MacroInput::Mouse(OnboardMouseButton::Left);
        for event in [
            MacroEvent::Press(ctrl_c),
            MacroEvent::Release(shift),
            MacroEvent::Press(click),
            MacroEvent::Delay(1500),
        ] {
            assert_eq!(event_from_device(event_to_device(event)), Some(event));
        }
        assert_eq!(
            event_to_device(MacroEvent::Press(click)),
            MacroCommand::ButtonDown(MouseButtons::LEFT)
        );
    }

    #[test]
    fn a_modifier_pressed_alone_reads_as_its_own_key() {
        let alone = event_from_device(MacroCommand::KeyPress {
            modifiers: KeyModifiers::LEFT_SHIFT,
            usage: 0,
        });
        assert_eq!(
            alone,
            Some(MacroEvent::Press(MacroInput::Key(HidKeyChord::key(
                HidKey::try_from(0xe1).unwrap()
            ))))
        );
        assert_eq!(
            event_from_device(MacroCommand::KeyPress {
                modifiers: KeyModifiers::LEFT_SHIFT | KeyModifiers::LEFT_CTRL,
                usage: 0,
            }),
            None
        );
        assert_eq!(
            event_from_device(MacroCommand::ButtonDown(
                MouseButtons::LEFT | MouseButtons::RIGHT
            )),
            None
        );
    }

    #[test]
    fn a_stroke_writes_as_press_wait_release() {
        let program = OnboardMacro {
            steps: vec![MacroStep::Stroke {
                input: MacroInput::Mouse(OnboardMouseButton::Right),
                hold_ms: 40,
            }],
        };
        let commands: Vec<_> = program.events().into_iter().map(event_to_device).collect();
        assert_eq!(
            commands,
            [
                MacroCommand::ButtonDown(MouseButtons::RIGHT),
                MacroCommand::Delay(40),
                MacroCommand::ButtonUp(MouseButtons::RIGHT),
            ]
        );
    }
}
