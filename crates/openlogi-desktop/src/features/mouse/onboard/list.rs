//! The Onboard Buttons card, for a mouse the onboard view has no drawing of:
//! the buttons of a G-series mouse's active HID++
//! `0x8100` onboard profile — what each does now, named by what its factory
//! profile makes it do — with a menu of the actions the mouse can run itself.
//!
//! Nothing is stored on this computer: the mouse keeps the bindings in its
//! active profile and runs them anywhere. A pick goes through
//! [`AppState::set_onboard_button`], which shows the action at once and lets
//! the mouse's read-back replace it.

use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div,
};
use gpui_component::{
    Disableable as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::DropdownMenu as _,
    v_flex,
};
use openlogi_core::hid::{OnboardBinding, OnboardButton, OnboardButtons};

use super::actions::{action_label, action_menu, binding_label, refusal_text};
use crate::state::{AppState, DeviceKey, OnboardButtonRefusal, OnboardButtonsLoad};
use crate::ui::components::PanelCard;
use crate::ui::status::{retry_line, status_line};
use crate::ui::theme::{Palette, Typography as _};

/// What the tab needs about the selected mouse.
pub(crate) struct OnboardButtonsFacts {
    key: DeviceKey,
    load: OnboardButtonsLoad,
    reachable: bool,
    refusal: Option<OnboardButtonRefusal>,
}

impl OnboardButtonsFacts {
    /// The selected mouse's onboard-button facts, or `None` when nothing is
    /// selected or the device has no onboard profile.
    pub(crate) fn current(cx: &App) -> Option<Self> {
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
pub(crate) fn onboard_buttons_card(
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

/// A button: its number and factory role on the left, its current action on
/// the right — a menu button, or, for the mouse's click buttons, a locked
/// label with a way back to the factory click if the profile lost it.
fn button_row(
    key: &DeviceKey,
    index: u8,
    button: &OnboardButton,
    reachable: bool,
    pal: Palette,
) -> impl IntoElement {
    let number = u16::from(index) + 1;
    let caption = if button.locked() {
        tr!("device.onboard_locked")
    } else {
        tr!("device.onboard_button_number", button => number)
    };
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
                        .child(caption),
                ),
        )
        .child(
            div()
                .flex_none()
                .child(action_control(key, index, button, reachable)),
        )
}

/// The row's control: the action menu, or for a locked click its current
/// action, disabled, unless the profile lost the click — then a button that
/// puts the factory click back.
fn action_control(
    key: &DeviceKey,
    index: u8,
    button: &OnboardButton,
    reachable: bool,
) -> AnyElement {
    let id = ("onboard-button-action", usize::from(index));
    if !button.locked() {
        return Button::new(id)
            .compact()
            .label(binding_label(&button.binding))
            .disabled(!reachable)
            .dropdown_menu_with_anchor(Anchor::TopRight, action_menu(key.clone(), index))
            .into_any_element();
    }
    match (&button.factory, &button.binding) {
        (Some(OnboardBinding::Action(factory)), current)
            if *current != OnboardBinding::Action(factory.clone()) =>
        {
            let key = key.clone();
            let factory = factory.clone();
            Button::new(id)
                .compact()
                .label(tr!("device.onboard_restore", action => action_label(&factory)))
                .disabled(!reachable)
                .on_click(move |_event, _window, cx| {
                    let factory = factory.clone();
                    AppState::apply(cx, |state| state.set_onboard_button(&key, index, factory));
                })
                .into_any_element()
        }
        _ => Button::new(id)
            .compact()
            .icon(Icon::empty().path("action-icons/lock.svg"))
            .label(binding_label(&button.binding))
            .disabled(true)
            .into_any_element(),
    }
}

/// What the row is called: the button's factory role ("DPI Shift"), which
/// names the physical button, or its number when the factory leaves it unset.
fn button_title(button: &OnboardButton, number: u16) -> SharedString {
    button.factory.as_ref().map_or_else(
        || tr!("device.onboard_button_number", button => number),
        binding_label,
    )
}
