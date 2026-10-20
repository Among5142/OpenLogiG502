//! Button bindings stored in a G-series mouse's onboard profile (HID++
//! `0x8100 OnboardProfiles`) — pure data, no I/O.
//!
//! Mice such as the G502 X LIGHTSPEED do not expose `0x1b04` ReprogControls;
//! each button's action lives in the active onboard profile, so it keeps
//! working with no software running and on any computer. The reads and
//! writes that produce and change an [`OnboardButtons`] live in
//! `openlogi_device::write::onboard_profiles`.
//!
//! Every enum here crosses the IPC boundary: variants are append-only.

use serde::{Deserialize, Serialize};

use super::HidKeyChord;

/// A standard mouse button an onboard binding can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnboardMouseButton {
    /// Button 1.
    Left,
    /// Button 2.
    Right,
    /// Button 3, the wheel click.
    Middle,
    /// Button 4.
    Back,
    /// Button 5.
    Forward,
}

/// A function the mouse firmware performs itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnboardFunction {
    /// Scroll left, as tilting the wheel does.
    TiltLeft,
    /// Scroll right, as tilting the wheel does.
    TiltRight,
    /// Step to the next DPI preset.
    NextDpi,
    /// Step to the previous DPI preset.
    PreviousDpi,
    /// Cycle through the DPI presets.
    CycleDpi,
    /// Return to the default DPI preset.
    DefaultDpi,
    /// Hold for the profile's shift DPI ("sniper").
    ShiftDpi,
    /// Switch to the next onboard profile.
    NextProfile,
    /// Switch to the previous onboard profile.
    PreviousProfile,
    /// Cycle through the onboard profiles.
    CycleProfile,
    /// Hold to use the profile's G-shift bindings.
    GShift,
    /// Show the battery level on the mouse's lights.
    BatteryStatus,
    /// Scroll down one notch.
    ScrollDown,
    /// Scroll up one notch.
    ScrollUp,
}

/// What software may assign to an onboard button: everything the mouse
/// runs itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnboardAction {
    /// The button does nothing.
    Disabled,
    /// Press a standard mouse button.
    Mouse(OnboardMouseButton),
    /// Press a key, with modifiers held around it. The mouse sends the raw
    /// HID report, so any keyboard-page key can be stored, a modifier on its
    /// own included.
    Key(HidKeyChord),
    /// Press a consumer-control (media) usage, HID page `0x0C`.
    Consumer(u16),
    /// A firmware function.
    Function(OnboardFunction),
}

/// What an onboard button is bound to, as read from the mouse.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnboardBinding {
    /// An action software can also write.
    Action(OnboardAction),
    /// A macro another program (G HUB) stored on the mouse. It can be shown
    /// and replaced, never written.
    Macro,
    /// Bytes no decoder recognises, shown raw.
    Unrecognized([u8; 4]),
}

/// One button of the active onboard profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardButton {
    /// What the button does now.
    pub binding: OnboardBinding,
    /// What the mouse's factory profile assigns it, which names the
    /// physical button; `None` when the factory profile leaves it unset.
    pub factory: Option<OnboardBinding>,
}

impl OnboardButton {
    /// Whether the button is the mouse's primary or secondary click — what
    /// its factory profile binds to Left or Right Click. Rebinding one could
    /// leave the mouse unable to click, so it only takes its factory action
    /// (see [`Self::accepts`]).
    #[must_use]
    pub fn locked(&self) -> bool {
        matches!(
            self.factory,
            Some(OnboardBinding::Action(OnboardAction::Mouse(
                OnboardMouseButton::Left | OnboardMouseButton::Right
            )))
        )
    }

    /// Whether `action` may be written to this button: anything, unless the
    /// button is [locked](Self::locked), which takes only its factory action
    /// — so a profile that already lost its click can get it back.
    #[must_use]
    pub fn accepts(&self, action: &OnboardAction) -> bool {
        !self.locked() || self.factory.as_ref() == Some(&OnboardBinding::Action(action.clone()))
    }
}

/// The buttons of a mouse's active onboard profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardButtons {
    /// The flash sector holding the active profile; it changes when the
    /// mouse switches profiles.
    pub sector: u16,
    /// One entry per programmable button, in the mouse's slot order.
    pub buttons: Vec<OnboardButton>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(factory: Option<OnboardAction>) -> OnboardButton {
        OnboardButton {
            binding: OnboardBinding::Action(OnboardAction::Disabled),
            factory: factory.map(OnboardBinding::Action),
        }
    }

    #[test]
    fn the_clicks_take_only_their_factory_action() {
        let left = button(Some(OnboardAction::Mouse(OnboardMouseButton::Left)));
        let right = button(Some(OnboardAction::Mouse(OnboardMouseButton::Right)));
        for (click, own) in [
            (&left, OnboardMouseButton::Left),
            (&right, OnboardMouseButton::Right),
        ] {
            assert!(click.locked());
            assert!(click.accepts(&OnboardAction::Mouse(own)));
            assert!(!click.accepts(&OnboardAction::Disabled));
            assert!(!click.accepts(&OnboardAction::Function(OnboardFunction::NextDpi)));
        }
        assert!(
            !left.accepts(&OnboardAction::Mouse(OnboardMouseButton::Right)),
            "swapping the clicks is a rebind too"
        );
    }

    #[test]
    fn every_other_button_takes_anything() {
        for factory in [
            Some(OnboardAction::Mouse(OnboardMouseButton::Middle)),
            Some(OnboardAction::Mouse(OnboardMouseButton::Back)),
            Some(OnboardAction::Function(OnboardFunction::ShiftDpi)),
            None,
        ] {
            let other = button(factory);
            assert!(!other.locked());
            assert!(other.accepts(&OnboardAction::Disabled));
            assert!(other.accepts(&OnboardAction::Mouse(OnboardMouseButton::Left)));
        }
    }
}
