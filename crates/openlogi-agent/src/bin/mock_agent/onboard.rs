//! Onboard buttons for a scripted mouse that reports an HID++ `0x8100`
//! onboard profile: the G502 X's factory layout, rebound in memory with the
//! same click lock the device layer applies.

use openlogi_hid::{
    OnboardAction, OnboardBinding, OnboardButton, OnboardButtons, OnboardFunction,
    OnboardMouseButton, WriteError,
};

/// The flash sector the scripted profile claims to live in: user profile 1.
const SECTOR: u16 = 0x0001;

/// The G502 X's factory bindings, in slot order, each bound to itself.
pub(super) fn stock_g502x() -> OnboardButtons {
    let factory = [
        OnboardAction::Mouse(OnboardMouseButton::Left),
        OnboardAction::Mouse(OnboardMouseButton::Right),
        OnboardAction::Mouse(OnboardMouseButton::Middle),
        OnboardAction::Mouse(OnboardMouseButton::Back),
        OnboardAction::Function(OnboardFunction::ShiftDpi),
        OnboardAction::Mouse(OnboardMouseButton::Forward),
        OnboardAction::Function(OnboardFunction::TiltLeft),
        OnboardAction::Function(OnboardFunction::TiltRight),
        OnboardAction::Function(OnboardFunction::CycleProfile),
        OnboardAction::Function(OnboardFunction::NextDpi),
        OnboardAction::Function(OnboardFunction::PreviousDpi),
    ];
    OnboardButtons {
        sector: SECTOR,
        buttons: factory
            .into_iter()
            .map(|action| OnboardButton {
                binding: OnboardBinding::Action(action.clone()),
                factory: Some(OnboardBinding::Action(action)),
            })
            .collect(),
    }
}

/// Bind `button` to `action`, refusing what the device layer refuses: a
/// button the profile does not have, and a click rebound away from itself.
pub(super) fn rebind(
    buttons: &mut OnboardButtons,
    button: u8,
    action: OnboardAction,
) -> Result<(), WriteError> {
    let entry = buttons
        .buttons
        .get_mut(usize::from(button))
        .ok_or(WriteError::OnboardButtonUnsupported { button })?;
    if !entry.accepts(&action) {
        return Err(WriteError::OnboardButtonLocked { button });
    }
    entry.binding = OnboardBinding::Action(action);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rebind_lands_and_a_click_stays_put() {
        let mut buttons = stock_g502x();
        let dpi_up = OnboardAction::Function(OnboardFunction::NextDpi);
        rebind(&mut buttons, 4, dpi_up.clone()).expect("DPI Shift takes anything");
        assert_eq!(buttons.buttons[4].binding, OnboardBinding::Action(dpi_up));
        assert_eq!(
            rebind(&mut buttons, 0, OnboardAction::Disabled),
            Err(WriteError::OnboardButtonLocked { button: 0 })
        );
        assert_eq!(
            rebind(&mut buttons, 11, OnboardAction::Disabled),
            Err(WriteError::OnboardButtonUnsupported { button: 11 })
        );
    }
}
