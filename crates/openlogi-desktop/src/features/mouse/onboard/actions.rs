//! What the onboard views say about a binding and how they change one: the
//! labels and icons of the actions the mouse runs itself, the menu of them,
//! and the dialog that binds a keyboard key.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, InteractiveElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div, px,
};
use gpui_component::{
    Selectable as _, WindowExt as _,
    button::Button,
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
    searchable_list::SearchableVec,
    select::{SelectItem, SelectState},
    v_flex,
};
use openlogi_core::hid::{
    HidKey, HidKeyChord, HidModifiers, OnboardAction, OnboardBinding, OnboardFunction,
    OnboardMouseButton, WriteError,
};

use crate::state::{AppState, DeviceKey, OnboardButtonRefusal};
use crate::ui::components::{control_select, save_cancel_footer};
use crate::ui::theme::{self, Typography as _};

/// The actions the mouse can run itself, grouped, then any keyboard key.
pub(super) fn action_menu(
    key: DeviceKey,
    button: u8,
) -> impl Fn(PopupMenu, &mut Window, &mut gpui::Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let pick = |action: OnboardAction| {
            let key = key.clone();
            PopupMenuItem::new(action_label(&action)).on_click(move |_, _, cx| {
                let action = action.clone();
                AppState::apply(cx, |state| state.set_onboard_button(&key, button, action));
            })
        };
        let mut menu = menu;
        for (i, (_, group)) in ACTION_GROUPS.iter().enumerate() {
            if i > 0 {
                menu = menu.item(PopupMenuItem::separator());
            }
            for action in *group {
                menu = menu.item(pick(action.clone()));
            }
        }
        let key = key.clone();
        menu.item(PopupMenuItem::separator()).item(
            PopupMenuItem::new(tr!("device.onboard_key")).on_click(move |_, window, cx| {
                open_key_dialog(window, cx, key.clone(), button);
            }),
        )
    }
}

/// The actions the mouse runs itself, grouped under the locale key of each
/// group's heading: mouse buttons, DPI, profiles, scrolling, media, then the
/// rest.
pub(super) const ACTION_GROUPS: [(&str, &[OnboardAction]); 6] = [
    (
        "device.mouse",
        &[
            OnboardAction::Mouse(OnboardMouseButton::Left),
            OnboardAction::Mouse(OnboardMouseButton::Right),
            OnboardAction::Mouse(OnboardMouseButton::Middle),
            OnboardAction::Mouse(OnboardMouseButton::Back),
            OnboardAction::Mouse(OnboardMouseButton::Forward),
        ],
    ),
    (
        "pointer.dpi",
        &[
            OnboardAction::Function(OnboardFunction::NextDpi),
            OnboardAction::Function(OnboardFunction::PreviousDpi),
            OnboardAction::Function(OnboardFunction::CycleDpi),
            OnboardAction::Function(OnboardFunction::DefaultDpi),
            OnboardAction::Function(OnboardFunction::ShiftDpi),
        ],
    ),
    (
        "device.onboard_group_profiles",
        &[
            OnboardAction::Function(OnboardFunction::NextProfile),
            OnboardAction::Function(OnboardFunction::PreviousProfile),
            OnboardAction::Function(OnboardFunction::CycleProfile),
            OnboardAction::Function(OnboardFunction::GShift),
        ],
    ),
    (
        "pointer.scroll",
        &[
            OnboardAction::Function(OnboardFunction::TiltLeft),
            OnboardAction::Function(OnboardFunction::TiltRight),
            OnboardAction::Function(OnboardFunction::ScrollUp),
            OnboardAction::Function(OnboardFunction::ScrollDown),
        ],
    ),
    (
        "actions.media",
        &[
            OnboardAction::Consumer(USAGE_PLAY_PAUSE),
            OnboardAction::Consumer(USAGE_NEXT_TRACK),
            OnboardAction::Consumer(USAGE_PREVIOUS_TRACK),
            OnboardAction::Consumer(USAGE_VOLUME_UP),
            OnboardAction::Consumer(USAGE_VOLUME_DOWN),
            OnboardAction::Consumer(USAGE_MUTE),
        ],
    ),
    (
        "device.onboard_group_other",
        // Battery Status is left out: on the G502 X it lights nothing. A
        // profile that has it still shows it, by name.
        &[OnboardAction::Disabled],
    ),
];

/// Consumer-page (`0x0C`) usages from the USB HID Usage Tables.
const USAGE_PLAY_PAUSE: u16 = 0x00cd;
const USAGE_NEXT_TRACK: u16 = 0x00b5;
const USAGE_PREVIOUS_TRACK: u16 = 0x00b6;
const USAGE_VOLUME_UP: u16 = 0x00e9;
const USAGE_VOLUME_DOWN: u16 = 0x00ea;
const USAGE_MUTE: u16 = 0x00e2;

/// What a binding read from the mouse shows as.
pub(super) fn binding_label(binding: &OnboardBinding) -> SharedString {
    match binding {
        OnboardBinding::Action(action) => action_label(action),
        OnboardBinding::Macro => tr!("device.onboard_macro"),
        OnboardBinding::Unrecognized(_) => tr!("device.onboard_unrecognized"),
    }
}

/// What an action shows as, reusing the action catalog's names where the
/// action is the same.
pub(super) fn action_label(action: &OnboardAction) -> SharedString {
    match action {
        OnboardAction::Disabled => tr!("device.onboard_disabled"),
        OnboardAction::Mouse(button) => match button {
            OnboardMouseButton::Left => tr!("actions.left_click"),
            OnboardMouseButton::Right => tr!("actions.right_click"),
            OnboardMouseButton::Middle => tr!("actions.middle_click"),
            OnboardMouseButton::Back => tr!("actions.back_button_4"),
            OnboardMouseButton::Forward => tr!("actions.forward_button_5"),
        },
        OnboardAction::Key(chord) => chord.to_string().into(),
        OnboardAction::Consumer(usage) => match *usage {
            USAGE_PLAY_PAUSE => tr!("actions.play_pause"),
            USAGE_NEXT_TRACK => tr!("actions.next_track"),
            USAGE_PREVIOUS_TRACK => tr!("actions.previous_track"),
            USAGE_VOLUME_UP => tr!("actions.volume_up"),
            USAGE_VOLUME_DOWN => tr!("actions.volume_down"),
            USAGE_MUTE => tr!("actions.mute"),
            other => tr!("device.onboard_media_key", usage => format!("0x{other:04X}")),
        },
        OnboardAction::Function(function) => match function {
            OnboardFunction::TiltLeft => tr!("actions.tilt_left"),
            OnboardFunction::TiltRight => tr!("actions.tilt_right"),
            OnboardFunction::ScrollUp => tr!("actions.scroll_up"),
            OnboardFunction::ScrollDown => tr!("actions.scroll_down"),
            OnboardFunction::NextDpi => tr!("device.onboard_next_dpi"),
            OnboardFunction::PreviousDpi => tr!("device.onboard_previous_dpi"),
            OnboardFunction::CycleDpi => tr!("device.onboard_cycle_dpi"),
            OnboardFunction::DefaultDpi => tr!("device.onboard_default_dpi"),
            OnboardFunction::ShiftDpi => tr!("device.onboard_shift_dpi"),
            OnboardFunction::NextProfile => tr!("device.onboard_next_profile"),
            OnboardFunction::PreviousProfile => tr!("device.onboard_previous_profile"),
            OnboardFunction::CycleProfile => tr!("device.onboard_cycle_profile"),
            OnboardFunction::GShift => tr!("device.onboard_g_shift"),
            OnboardFunction::BatteryStatus => tr!("device.onboard_battery_status"),
        },
    }
}

/// The glyph a binding read from the mouse shows with, from the action
/// icons the button view uses.
pub(super) fn binding_icon(binding: &OnboardBinding) -> &'static str {
    let OnboardBinding::Action(action) = binding else {
        return "action-icons/scroll-text.svg";
    };
    match action {
        OnboardAction::Disabled => "action-icons/ban.svg",
        OnboardAction::Mouse(OnboardMouseButton::Left | OnboardMouseButton::Right) => {
            "action-icons/mouse-pointer-click.svg"
        }
        OnboardAction::Mouse(OnboardMouseButton::Middle) => "action-icons/mouse.svg",
        OnboardAction::Mouse(OnboardMouseButton::Back) => "action-icons/circle-arrow-left.svg",
        OnboardAction::Mouse(OnboardMouseButton::Forward) => "action-icons/circle-arrow-right.svg",
        OnboardAction::Key(_) => "action-icons/keyboard.svg",
        OnboardAction::Consumer(usage) => match *usage {
            USAGE_PLAY_PAUSE => "action-icons/play.svg",
            USAGE_NEXT_TRACK => "action-icons/skip-forward.svg",
            USAGE_PREVIOUS_TRACK => "action-icons/skip-back.svg",
            USAGE_VOLUME_UP => "action-icons/volume-2.svg",
            USAGE_VOLUME_DOWN => "action-icons/volume-1.svg",
            USAGE_MUTE => "action-icons/volume-x.svg",
            _ => "action-icons/keyboard.svg",
        },
        OnboardAction::Function(function) => match function {
            OnboardFunction::TiltLeft => "action-icons/chevrons-left.svg",
            OnboardFunction::TiltRight => "action-icons/chevrons-right.svg",
            OnboardFunction::ScrollUp => "action-icons/chevrons-up.svg",
            OnboardFunction::ScrollDown => "action-icons/chevrons-down.svg",
            OnboardFunction::NextDpi
            | OnboardFunction::PreviousDpi
            | OnboardFunction::CycleDpi
            | OnboardFunction::DefaultDpi
            | OnboardFunction::ShiftDpi => "action-icons/gauge.svg",
            OnboardFunction::NextProfile
            | OnboardFunction::PreviousProfile
            | OnboardFunction::CycleProfile
            | OnboardFunction::GShift => "action-icons/layers.svg",
            OnboardFunction::BatteryStatus => "action-icons/battery-low.svg",
        },
    }
}

pub(super) fn refusal_text(refusal: &OnboardButtonRefusal) -> SharedString {
    let button = u16::from(refusal.button) + 1;
    match refusal.error {
        WriteError::OnboardProfileNotWritable => tr!("device.onboard_profile_not_writable"),
        WriteError::OnboardBindingNotApplied { .. } => {
            tr!("device.onboard_binding_not_kept", button => button)
        }
        WriteError::OnboardButtonLocked { .. } => {
            tr!("device.onboard_button_locked", button => button)
        }
        _ => tr!("device.onboard_binding_failed", button => button),
    }
}

/// A key in the picker: its name, searchable, and its usage.
#[derive(Clone)]
pub(super) struct KeyOption(pub(super) HidKey);

impl SelectItem for KeyOption {
    type Value = HidKey;

    fn title(&self) -> SharedString {
        SharedString::new_static(self.0.name())
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

/// The modifiers the dialog offers as toggles, left-hand only: the mouse
/// replays them as the computer's own, and a right-hand modifier is still
/// one pick away as a key of its own.
pub(super) const MODIFIER_TOGGLES: [(HidModifiers, &str); 4] = [
    (HidModifiers::LEFT_CTRL, "Ctrl"),
    (HidModifiers::LEFT_SHIFT, "Shift"),
    (HidModifiers::LEFT_ALT, "Alt"),
    (HidModifiers::LEFT_GUI, "Win"),
];

/// The chord the dialog's picks make, or `None` until a key is chosen.
pub(super) fn picked_chord(modifiers: HidModifiers, key: Option<HidKey>) -> Option<HidKeyChord> {
    key.map(|key| HidKeyChord { modifiers, key })
}

fn open_key_dialog(window: &mut Window, cx: &mut App, key: DeviceKey, button: u8) {
    let number = u16::from(button) + 1;
    let options: Vec<KeyOption> = HidKey::standard().map(KeyOption).collect();
    let keys = cx
        .new(|cx| SelectState::new(SearchableVec::new(options), None, window, cx).searchable(true));
    let modifiers = Rc::new(Cell::new(HidModifiers::NONE));
    // Set once Save is pressed with no key chosen, so the caption says why
    // Save did nothing.
    let tried = Rc::new(Cell::new(false));
    window.open_dialog(cx, move |dialog, _window, cx| {
        let pal = theme::palette(cx);
        let chosen = keys.read(cx).selected_value().copied();
        let caption = match picked_chord(modifiers.get(), chosen) {
            Some(chord) => SharedString::from(chord.to_string()),
            None if tried.get() => tr!("device.onboard_key_choose"),
            None => tr!("device.onboard_key_help"),
        };
        let toggles =
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(MODIFIER_TOGGLES.map(|(flag, name)| {
                    let modifiers = modifiers.clone();
                    Button::new(SharedString::new_static(name))
                        .compact()
                        .label(name)
                        .selected(modifiers.get().contains(flag))
                        .on_click(move |_event, window, _cx| {
                            let current = modifiers.get();
                            modifiers.set(if current.contains(flag) {
                                current.without(flag)
                            } else {
                                current.with(flag)
                            });
                            window.refresh();
                        })
                }));
        dialog
            .w(px(440.))
            .title(tr!("device.onboard_key_title", button => number))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(tr!("device.onboard_key_hold")),
                            )
                            .child(toggles),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(tr!("device.onboard_key_press")),
                            )
                            .child(
                                control_select(&keys).placeholder(tr!("device.onboard_key_search")),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "onboard-key-caption".into())
                            .text_caption()
                            .text_color(pal.text_muted)
                            .child(caption),
                    ),
            )
            .footer(save_cancel_footer())
            .on_ok({
                let keys = keys.clone();
                let modifiers = modifiers.clone();
                let key = key.clone();
                let tried = tried.clone();
                move |_, window, cx| {
                    let chosen = keys.read(cx).selected_value().copied();
                    let Some(chord) = picked_chord(modifiers.get(), chosen) else {
                        tried.set(true);
                        window.refresh();
                        return false;
                    };
                    AppState::apply(cx, |state| {
                        state.set_onboard_button(&key, button, OnboardAction::Key(chord))
                    });
                    true
                }
            })
    });
}

#[cfg(test)]
mod tests {
    use openlogi_core::hid::{
        HidKey, HidModifiers, OnboardAction, OnboardBinding, OnboardFunction,
    };

    use super::{ACTION_GROUPS, binding_label, picked_chord};
    use crate::services::i18n::LOCALE_LOCK;

    #[test]
    fn the_picks_make_a_chord_once_a_key_is_chosen() {
        let insert = HidKey::try_from(0x49).expect("Insert");
        assert_eq!(picked_chord(HidModifiers::LEFT_CTRL, None), None);
        let chord = picked_chord(HidModifiers::LEFT_CTRL, Some(insert)).expect("a chord");
        assert_eq!(chord.to_string(), "Ctrl+Insert");
        let shift = HidKey::try_from(0xe1).expect("Left Shift");
        assert_eq!(
            picked_chord(HidModifiers::NONE, Some(shift))
                .expect("a chord")
                .to_string(),
            "Left Shift"
        );
    }

    #[test]
    fn the_key_picker_offers_each_standard_key_once() {
        assert_eq!(
            HidKey::standard().count(),
            HidKey::standard()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
        assert!(HidKey::standard().any(|key| key.name() == "Print Screen"));
        assert!(HidKey::standard().any(|key| key.name() == "Keypad 7"));
        assert!(HidKey::standard().any(|key| key.name() == "F24"));
    }

    #[test]
    fn the_menu_offers_each_action_once() {
        let mut seen = Vec::new();
        for action in ACTION_GROUPS.iter().flat_map(|(_, group)| group.iter()) {
            assert!(!seen.contains(&action), "{action:?} offered twice");
            seen.push(action);
        }
        assert!(seen.contains(&&OnboardAction::Disabled));
        assert!(seen.contains(&&OnboardAction::Function(OnboardFunction::ShiftDpi)));
        assert!(
            !seen.contains(&&OnboardAction::Function(OnboardFunction::BatteryStatus)),
            "the G502 X has no lights to show the battery on"
        );
    }

    #[test]
    fn bindings_read_from_the_mouse_are_labelled() {
        let _guard = LOCALE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rust_i18n::set_locale("en");
        assert_eq!(binding_label(&OnboardBinding::Macro), "G HUB macro");
        assert_eq!(
            binding_label(&OnboardBinding::Action(OnboardAction::Function(
                OnboardFunction::ShiftDpi
            ))),
            "DPI Shift"
        );
        assert_eq!(
            binding_label(&OnboardBinding::Action(OnboardAction::Consumer(0x00e9))),
            "Volume Up"
        );
        assert_eq!(
            binding_label(&OnboardBinding::Action(OnboardAction::Consumer(0x0183))),
            "Media key 0x0183"
        );
    }
}
