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
    /// Run a macro the mouse stores itself.
    Macro(OnboardMacro),
}

/// What a macro step presses: a key, with modifiers held around it, or a
/// standard mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MacroInput {
    /// A key; a modifier on its own is the modifier's own key.
    Key(HidKeyChord),
    /// A mouse button.
    Mouse(OnboardMouseButton),
}

/// One step of a macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MacroStep {
    /// Press an input, hold it `hold_ms` milliseconds, release it.
    Stroke {
        /// What is pressed.
        input: MacroInput,
        /// How long it is held, in milliseconds.
        hold_ms: u16,
    },
    /// Press an input and keep it held — a recorded press another input
    /// overlaps, released by a later [`Self::Release`].
    Press(MacroInput),
    /// Release a held input.
    Release(MacroInput),
    /// Wait, in milliseconds.
    Delay(u16),
}

/// A macro: steps the mouse runs in order when the button is pressed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OnboardMacro {
    /// The steps, in order.
    pub steps: Vec<MacroStep>,
}

/// One input going down or up, the unit a macro is stored and recorded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacroEvent {
    /// The input goes down.
    Press(MacroInput),
    /// The input comes up.
    Release(MacroInput),
    /// Wait, in milliseconds.
    Delay(u16),
}

impl OnboardMacro {
    /// The macro from raw events — what the mouse stores, or what a
    /// recording captured. A press, one wait and the release of the same
    /// input fold into a [`MacroStep::Stroke`]; anything else stays a press,
    /// a release or a wait.
    #[must_use]
    pub fn from_events(events: &[MacroEvent]) -> Self {
        let mut steps = Vec::with_capacity(events.len());
        let mut at = 0;
        while let Some(event) = events.get(at) {
            if let (
                MacroEvent::Press(input),
                Some(MacroEvent::Delay(hold_ms)),
                Some(MacroEvent::Release(released)),
            ) = (event, events.get(at + 1), events.get(at + 2))
                && input == released
            {
                steps.push(MacroStep::Stroke {
                    input: *input,
                    hold_ms: *hold_ms,
                });
                at += 3;
                continue;
            }
            steps.push(match *event {
                MacroEvent::Press(input) => MacroStep::Press(input),
                MacroEvent::Release(input) => MacroStep::Release(input),
                MacroEvent::Delay(ms) => MacroStep::Delay(ms),
            });
            at += 1;
        }
        Self { steps }
    }

    /// The raw events the steps run as: a stroke is a press, a wait and a
    /// release. The inverse of [`Self::from_events`].
    #[must_use]
    pub fn events(&self) -> Vec<MacroEvent> {
        let mut events = Vec::with_capacity(self.steps.len() * 3);
        for step in &self.steps {
            match *step {
                MacroStep::Stroke { input, hold_ms } => events.extend([
                    MacroEvent::Press(input),
                    MacroEvent::Delay(hold_ms),
                    MacroEvent::Release(input),
                ]),
                MacroStep::Press(input) => events.push(MacroEvent::Press(input)),
                MacroStep::Release(input) => events.push(MacroEvent::Release(input)),
                MacroStep::Delay(ms) => events.push(MacroEvent::Delay(ms)),
            }
        }
        events
    }
}

/// What an onboard button is bound to, as read from the mouse.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnboardBinding {
    /// An action software can also write.
    Action(OnboardAction),
    /// A macro stored on the mouse with commands this app does not read (a
    /// jump, a repeat, a media key…). It can be shown and replaced, never
    /// edited; a macro the app reads is an [`OnboardAction::Macro`].
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

    fn key(usage: u8) -> MacroInput {
        MacroInput::Key(HidKeyChord::key(
            super::super::HidKey::try_from(usage).expect("a named usage"),
        ))
    }

    #[test]
    fn a_press_wait_release_of_one_input_folds_into_a_stroke() {
        let (three, space, x) = (key(0x20), key(0x2c), key(0x1b));
        // G HUB's "tap 3, then Space+X": the 3 is a stroke, the overlapping
        // Space and X stay presses and releases.
        let events = [
            MacroEvent::Press(three),
            MacroEvent::Delay(70),
            MacroEvent::Release(three),
            MacroEvent::Delay(200),
            MacroEvent::Press(space),
            MacroEvent::Delay(65),
            MacroEvent::Press(x),
            MacroEvent::Delay(76),
            MacroEvent::Release(space),
            MacroEvent::Delay(55),
            MacroEvent::Release(x),
        ];
        let folded = OnboardMacro::from_events(&events);
        assert_eq!(
            folded.steps,
            [
                MacroStep::Stroke {
                    input: three,
                    hold_ms: 70
                },
                MacroStep::Delay(200),
                MacroStep::Press(space),
                MacroStep::Delay(65),
                MacroStep::Press(x),
                MacroStep::Delay(76),
                MacroStep::Release(space),
                MacroStep::Delay(55),
                MacroStep::Release(x),
            ]
        );
        assert_eq!(folded.events(), events);
    }

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
