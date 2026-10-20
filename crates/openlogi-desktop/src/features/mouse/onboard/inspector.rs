//! The fixed panel beside the onboard diagram: what the selected button is,
//! what it does now, and how to change it.

use gpui::{
    Anchor, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _,
    div, svg,
};
use gpui_component::{
    Disableable as _, Icon, button::Button, h_flex, menu::DropdownMenu as _, v_flex,
};
use openlogi_core::hid::{OnboardBinding, OnboardButton};

use super::super::inspector::{inspector_heading, inspector_panel};
use super::actions::{action_label, action_menu, binding_icon, binding_label, refusal_text};
use super::shape::OnboardTarget;
use super::{OnboardDrawing, button_name};
use crate::state::AppState;
use crate::ui::theme::{Palette, Typography as _};

/// The inspector for `selected`, or the prompt to pick a button.
pub(super) fn onboard_inspector(
    drawing: &OnboardDrawing,
    selected: Option<OnboardTarget>,
    pal: Palette,
) -> gpui::Div {
    let body = match selected.and_then(|target| Some((target, drawing.button(target)?))) {
        Some((target, button)) => button_inspector(drawing, target, button, pal).into_any_element(),
        None => v_flex()
            .gap_3()
            .child(inspector_heading(
                tr!("actions.button_inspector"),
                None,
                pal,
            ))
            .child(
                div()
                    .text_body()
                    .text_color(pal.text_muted)
                    .child(tr!("profiles.select_device_button_description")),
            )
            .into_any_element(),
    };
    inspector_panel("onboard-inspector-scroll", body, pal)
}

fn button_inspector(
    drawing: &OnboardDrawing,
    target: OnboardTarget,
    button: &OnboardButton,
    pal: Palette,
) -> impl IntoElement {
    let number = tr!(
        "device.onboard_button_number",
        button => u16::from(target.slot) + 1
    );
    let status: SharedString = match drawing.shape.printed_name(target.role) {
        Some(printed) => format!("{printed} · {number}").into(),
        None => number,
    };
    let refusal = drawing
        .refusal
        .as_ref()
        .filter(|refusal| refusal.button == target.slot);
    v_flex()
        .gap_4()
        .child(inspector_heading(button_name(button), Some(status), pal))
        .child(current_action(&button.binding, pal))
        .child(change_control(drawing, target, button, pal))
        .children(refusal.map(|refusal| {
            div()
                .debug_selector(|| "onboard-button-refusal".into())
                .text_caption()
                .text_color(pal.text_muted)
                .child(refusal_text(refusal))
        }))
}

/// What the button does now, read back from the mouse.
fn current_action(binding: &OnboardBinding, pal: Palette) -> impl IntoElement {
    v_flex()
        .gap_2()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(pal.border)
        .bg(pal.control)
        .p_3()
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(tr!("actions.current_action")),
        )
        .child(
            h_flex()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    svg()
                        .path(binding_icon(binding))
                        .size_4()
                        .flex_none()
                        .text_color(pal.text_muted),
                )
                .child(
                    div()
                        .debug_selector(|| "onboard-current-action".into())
                        .min_w_0()
                        .truncate()
                        .text_body()
                        .child(binding_label(binding)),
                ),
        )
}

/// How to change it: the action menu, or for a click button why it is
/// locked, with a way back to its factory click if the profile lost it.
fn change_control(
    drawing: &OnboardDrawing,
    target: OnboardTarget,
    button: &OnboardButton,
    pal: Palette,
) -> impl IntoElement {
    let slot = target.slot;
    if !button.locked() {
        return Button::new("onboard-change-action")
            .outline()
            .w_full()
            .label(tr!("device.onboard_change_action"))
            .disabled(!drawing.reachable)
            .dropdown_menu_with_anchor(Anchor::TopLeft, action_menu(drawing.key.clone(), slot))
            .into_any_element();
    }
    let restore = match (&button.factory, &button.binding) {
        (Some(OnboardBinding::Action(factory)), current)
            if *current != OnboardBinding::Action(factory.clone()) =>
        {
            let key = drawing.key.clone();
            let factory = factory.clone();
            Some(
                Button::new("onboard-restore")
                    .outline()
                    .w_full()
                    .label(tr!("device.onboard_restore", action => action_label(&factory)))
                    .disabled(!drawing.reachable)
                    .on_click(move |_event, _window, cx| {
                        let factory = factory.clone();
                        AppState::apply(cx, |state| state.set_onboard_button(&key, slot, factory));
                    }),
            )
        }
        _ => None,
    };
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::empty()
                        .path("action-icons/lock.svg")
                        .size_4()
                        .text_color(pal.text_muted),
                )
                .child(
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(tr!("device.onboard_locked")),
                ),
        )
        .children(restore)
        .into_any_element()
}
