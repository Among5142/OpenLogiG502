//! The Onboard Buttons tab: the buttons of a G-series mouse's active HID++
//! `0x8100` onboard profile — what each does now, named by what its factory
//! profile makes it do — with a menu of the actions the mouse can run itself.
//!
//! Nothing is stored on this computer: the mouse keeps the bindings in its
//! active profile and runs them anywhere. A pick goes through
//! [`AppState::set_onboard_button`], which shows the action at once and lets
//! the mouse's read-back replace it.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, App, AppContext as _, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use gpui_component::{
    Disableable as _, Icon, IconName, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::InputState,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use openlogi_core::binding::KeyCombo;
use openlogi_core::hid::{
    OnboardAction, OnboardBinding, OnboardButton, OnboardButtons, OnboardFunction,
    OnboardMouseButton, WriteError,
};

use crate::state::{AppState, DeviceKey, OnboardButtonRefusal, OnboardButtonsLoad};
use crate::ui::components::{PanelCard, control_input, save_cancel_footer};
use crate::ui::status::{retry_line, status_line};
use crate::ui::theme::{self, Palette, Typography as _};

/// What the tab needs about the selected mouse.
pub(super) struct OnboardButtonsFacts {
    key: DeviceKey,
    load: OnboardButtonsLoad,
    reachable: bool,
    refusal: Option<OnboardButtonRefusal>,
}

impl OnboardButtonsFacts {
    /// The selected mouse's onboard-button facts, or `None` when nothing is
    /// selected or the device has no onboard profile.
    pub(super) fn current(cx: &App) -> Option<Self> {
        let state = AppState::try_read(cx)?;
        if !state.current_onboard_buttons_supported() {
            return None;
        }
        let record = state.current_record()?;
        Some(Self {
            key: record.device_key(),
            load: state.current_onboard_buttons_load(),
            reachable: state.current_onboard_buttons_reachable(),
            refusal: state.current_onboard_button_refusal(),
        })
    }
}

/// The card, or `None` when the mouse has no onboard profile to show.
pub(super) fn onboard_buttons_card(
    facts: Option<OnboardButtonsFacts>,
    pal: Palette,
) -> Option<AnyElement> {
    let OnboardButtonsFacts {
        key,
        load,
        reachable,
        refusal,
    } = facts?;
    let body: AnyElement = match load {
        OnboardButtonsLoad::Ready(buttons) => {
            button_rows(&key, &buttons, reachable, refusal.as_ref(), pal).into_any_element()
        }
        OnboardButtonsLoad::Unknown | OnboardButtonsLoad::Loading if !reachable => return None,
        OnboardButtonsLoad::Unknown | OnboardButtonsLoad::Loading => {
            status_line(tr!("device.reading_onboard_buttons"), pal).into_any_element()
        }
        OnboardButtonsLoad::Failed(_) => retry_line(
            "onboard-buttons-retry",
            tr!("device.couldnt_read_onboard_buttons_click_to_retry"),
            pal,
            move |cx| AppState::apply(cx, |state| state.refresh_onboard_buttons(&key)),
        )
        .into_any_element(),
        OnboardButtonsLoad::Unsupported(_) => div()
            .text_caption()
            .text_color(pal.text_muted)
            .child(tr!("device.onboard_profile_not_writable"))
            .into_any_element(),
    };
    Some(
        PanelCard::new(
            tr!("device.onboard_buttons"),
            Icon::empty().path("action-icons/mouse-pointer-click.svg"),
            body,
        )
        .into_any_element(),
    )
}

/// The profile line, one row per button, then why the last write did not
/// land.
fn button_rows(
    key: &DeviceKey,
    buttons: &Arc<OnboardButtons>,
    reachable: bool,
    refusal: Option<&OnboardButtonRefusal>,
    pal: Palette,
) -> impl IntoElement {
    let refresh_key = key.clone();
    v_flex()
        .gap_3()
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div().text_body().text_color(pal.text_primary).child(
                                tr!("device.onboard_profile_number", profile => buttons.sector),
                            ),
                        )
                        .child(
                            div()
                                .text_caption()
                                .text_color(pal.text_muted)
                                .child(tr!("device.onboard_buttons_description")),
                        ),
                )
                .child(
                    Button::new("onboard-buttons-refresh")
                        .ghost()
                        .compact()
                        .icon(IconName::Redo)
                        .disabled(!reachable)
                        .on_click(move |_event, _window, cx| {
                            AppState::apply(cx, |state| {
                                state.refresh_onboard_buttons(&refresh_key)
                            });
                        }),
                ),
        )
        .children(
            buttons
                .buttons
                .iter()
                .enumerate()
                .filter_map(|(index, button)| {
                    let index = u8::try_from(index).ok()?;
                    Some(button_row(key, index, button, reachable, pal))
                }),
        )
        .children(refusal.map(|refusal| {
            div()
                .debug_selector(|| "onboard-button-refusal".into())
                .text_caption()
                .text_color(pal.text_muted)
                .child(refusal_text(refusal))
        }))
}

/// A button: its number and factory role on the left, its current action as
/// a menu button on the right.
fn button_row(
    key: &DeviceKey,
    index: u8,
    button: &OnboardButton,
    reachable: bool,
    pal: Palette,
) -> impl IntoElement {
    let number = u16::from(index) + 1;
    let menu_key = key.clone();
    h_flex()
        .id(("onboard-button", usize::from(index)))
        .debug_selector(move || format!("onboard-button-{index}"))
        .w_full()
        .min_w_0()
        .gap_3()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_body()
                        .text_color(pal.text_primary)
                        .child(button_title(button, number)),
                )
                .child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(tr!("device.onboard_button_number", button => number)),
                ),
        )
        .child(
            div().flex_none().child(
                Button::new(("onboard-button-action", usize::from(index)))
                    .compact()
                    .label(binding_label(&button.binding))
                    .disabled(!reachable)
                    .dropdown_menu_with_anchor(Anchor::TopRight, action_menu(menu_key, index)),
            ),
        )
}

/// What the row is called: the button's factory role ("DPI Shift"), which
/// names the physical button, or its number when the factory leaves it unset.
fn button_title(button: &OnboardButton, number: u16) -> SharedString {
    button.factory.as_ref().map_or_else(
        || tr!("device.onboard_button_number", button => number),
        binding_label,
    )
}

/// The actions the mouse can run itself, grouped, then a keyboard shortcut.
fn action_menu(
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
        for (i, group) in MENU_GROUPS.iter().enumerate() {
            if i > 0 {
                menu = menu.item(PopupMenuItem::separator());
            }
            for action in *group {
                menu = menu.item(pick(action.clone()));
            }
        }
        let key = key.clone();
        menu.item(PopupMenuItem::separator()).item(
            PopupMenuItem::new(tr!("device.onboard_keyboard_shortcut")).on_click(
                move |_, window, cx| {
                    open_shortcut_dialog(window, cx, key.clone(), button);
                },
            ),
        )
    }
}

/// The menu's action groups: mouse buttons, DPI, profiles, scrolling, media,
/// then the rest.
const MENU_GROUPS: [&[OnboardAction]; 6] = [
    &[
        OnboardAction::Mouse(OnboardMouseButton::Left),
        OnboardAction::Mouse(OnboardMouseButton::Right),
        OnboardAction::Mouse(OnboardMouseButton::Middle),
        OnboardAction::Mouse(OnboardMouseButton::Back),
        OnboardAction::Mouse(OnboardMouseButton::Forward),
    ],
    &[
        OnboardAction::Function(OnboardFunction::NextDpi),
        OnboardAction::Function(OnboardFunction::PreviousDpi),
        OnboardAction::Function(OnboardFunction::CycleDpi),
        OnboardAction::Function(OnboardFunction::DefaultDpi),
        OnboardAction::Function(OnboardFunction::ShiftDpi),
    ],
    &[
        OnboardAction::Function(OnboardFunction::NextProfile),
        OnboardAction::Function(OnboardFunction::PreviousProfile),
        OnboardAction::Function(OnboardFunction::CycleProfile),
        OnboardAction::Function(OnboardFunction::GShift),
    ],
    &[
        OnboardAction::Function(OnboardFunction::TiltLeft),
        OnboardAction::Function(OnboardFunction::TiltRight),
        OnboardAction::Function(OnboardFunction::ScrollUp),
        OnboardAction::Function(OnboardFunction::ScrollDown),
    ],
    &[
        OnboardAction::Consumer(USAGE_PLAY_PAUSE),
        OnboardAction::Consumer(USAGE_NEXT_TRACK),
        OnboardAction::Consumer(USAGE_PREVIOUS_TRACK),
        OnboardAction::Consumer(USAGE_VOLUME_UP),
        OnboardAction::Consumer(USAGE_VOLUME_DOWN),
        OnboardAction::Consumer(USAGE_MUTE),
    ],
    &[
        OnboardAction::Function(OnboardFunction::BatteryStatus),
        OnboardAction::Disabled,
    ],
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
        OnboardAction::Key(combo) => combo.rendered_label().into(),
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

fn refusal_text(refusal: &OnboardButtonRefusal) -> SharedString {
    let button = u16::from(refusal.button) + 1;
    match refusal.error {
        WriteError::OnboardProfileNotWritable => tr!("device.onboard_profile_not_writable"),
        WriteError::OnboardBindingNotApplied { .. } => {
            tr!("device.onboard_binding_not_kept", button => button)
        }
        _ => tr!("device.onboard_binding_failed", button => button),
    }
}

/// The chord a typed shortcut names, or `None` for one the mouse cannot
/// store.
fn parse_shortcut(value: &str) -> Option<KeyCombo> {
    value.trim().parse().ok()
}

fn open_shortcut_dialog(window: &mut Window, cx: &mut App, key: DeviceKey, button: u8) {
    let number = u16::from(button) + 1;
    let input = cx.new(|cx| {
        InputState::new(window, cx).placeholder(tr!("device.onboard_shortcut_placeholder"))
    });
    // Set once Save is pressed on text that is not a shortcut, so the
    // caption says why Save did nothing.
    let tried = Rc::new(Cell::new(false));
    window.open_dialog(cx, move |dialog, window, cx| {
        input.update(cx, |input, cx| input.focus(window, cx));
        let pal = theme::palette(cx);
        let typed = input.read(cx).value().to_string();
        let caption = if tried.get() && parse_shortcut(&typed).is_none() {
            tr!("device.onboard_shortcut_invalid")
        } else {
            tr!("device.onboard_shortcut_help")
        };
        dialog
            .w(px(420.))
            .title(tr!("device.onboard_shortcut_title", button => number))
            .child(
                v_flex().gap_2().child(control_input(&input)).child(
                    div()
                        .debug_selector(|| "onboard-shortcut-caption".into())
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(caption),
                ),
            )
            .footer(save_cancel_footer())
            .on_ok({
                let input = input.clone();
                let key = key.clone();
                let tried = tried.clone();
                move |_, window, cx| {
                    let Some(combo) = parse_shortcut(&input.read(cx).value()) else {
                        tried.set(true);
                        window.refresh();
                        return false;
                    };
                    AppState::apply(cx, |state| {
                        state.set_onboard_button(&key, button, OnboardAction::Key(combo))
                    });
                    true
                }
            })
    });
}

#[cfg(test)]
mod tests {
    use openlogi_core::hid::{OnboardAction, OnboardBinding, OnboardFunction};

    use super::{MENU_GROUPS, binding_label, parse_shortcut};
    use crate::services::i18n::LOCALE_LOCK;

    #[test]
    fn typed_shortcuts_parse_and_garbage_does_not() {
        let combo = parse_shortcut(" Ctrl+Shift+C ").expect("a valid chord");
        assert_eq!(combo.rendered_label(), "Ctrl+Shift+C");
        for invalid in ["", "Ctrl", "Ctrl+P+K", "Hyper+C"] {
            assert!(parse_shortcut(invalid).is_none(), "{invalid:?} parsed");
        }
    }

    #[test]
    fn the_menu_offers_each_action_once() {
        let mut seen = Vec::new();
        for action in MENU_GROUPS.iter().flat_map(|group| group.iter()) {
            assert!(!seen.contains(&action), "{action:?} offered twice");
            seen.push(action);
        }
        assert!(seen.contains(&&OnboardAction::Disabled));
        assert!(seen.contains(&&OnboardAction::Function(OnboardFunction::ShiftDpi)));
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
