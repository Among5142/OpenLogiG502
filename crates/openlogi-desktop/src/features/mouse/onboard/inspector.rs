//! The fixed panel beside the onboard diagram: what the selected button is,
//! what it does now, and how to change it.

use gpui::{
    Entity, InteractiveElement as _, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, rgb, svg,
};
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use openlogi_core::hid::{HidKey, OnboardAction, OnboardBinding, OnboardButton};

use super::super::inspector::{inspector_heading, inspector_panel};
use super::actions::{
    ACTION_GROUPS, MODIFIER_TOGGLES, action_label, binding_icon, binding_label, picked_chord,
    refusal_text,
};
use super::macro_editor::{MacroDraft, macro_editor};
use super::shape::OnboardTarget;
use super::{OnboardDrawing, OnboardModelView, PickMode, PickerState, button_name};
use crate::features::binding_editor::editor_section;
use crate::state::{AppState, DeviceKey};
use crate::ui::components::{MenuRow, control_button, control_select};
use crate::ui::theme::{ACCENT_BLUE, Palette, Typography as _};

/// What the picker shows: its state, the macro draft, and the key the key
/// search holds.
pub(super) struct Picking<'a> {
    pub(super) picker: &'a PickerState,
    pub(super) draft: &'a MacroDraft,
    pub(super) chosen_key: Option<HidKey>,
}

/// The inspector for `selected`, or the prompt to pick a button.
pub(super) fn onboard_inspector(
    drawing: &OnboardDrawing,
    selected: Option<OnboardTarget>,
    picking: &Picking<'_>,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> gpui::Div {
    let body = match selected.and_then(|target| Some((target, drawing.button(target)?))) {
        Some((target, button)) => {
            button_inspector(drawing, target, button, picking, view, pal).into_any_element()
        }
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
    picking: &Picking<'_>,
    view: &Entity<OnboardModelView>,
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
        .children(refusal.map(|refusal| {
            div()
                .debug_selector(|| "onboard-button-refusal".into())
                .text_caption()
                .text_color(pal.text_muted)
                .child(refusal_text(refusal))
        }))
        .child(if button.locked() {
            locked_control(drawing, target, button, pal).into_any_element()
        } else {
            picker_control(drawing, target, button, picking, view, pal).into_any_element()
        })
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

/// How to change an unlocked button: the kind of binding, then the actions
/// the mouse runs itself, a key to send, or a macro.
fn picker_control(
    drawing: &OnboardDrawing,
    target: OnboardTarget,
    button: &OnboardButton,
    picking: &Picking<'_>,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    let Picking {
        picker, chosen_key, ..
    } = *picking;
    let mode_button = |id: &'static str, label: SharedString, mode: PickMode| {
        let view = view.clone();
        control_button(id)
            .debug_selector(move || id.to_string())
            .flex_1()
            .label(label)
            // `selected` alone barely tints a button; the accent shows the
            // pick at a glance.
            .when(mode == picker.mode, ButtonVariants::primary)
            .on_click(move |_, _, cx| {
                view.update(cx, |view, cx| {
                    view.set_pick_mode(mode);
                    cx.notify();
                });
            })
    };
    let body = match picker.mode {
        PickMode::Actions => action_list(
            &drawing.key,
            target.slot,
            &button.binding,
            drawing.reachable,
            pal,
        )
        .into_any_element(),
        PickMode::Key => {
            key_picker(drawing, target, picker, chosen_key, view, pal).into_any_element()
        }
        PickMode::Macro => {
            macro_editor(drawing, target, &button.binding, picking, view, pal).into_any_element()
        }
    };
    v_flex()
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .child(mode_button(
                    "onboard-mode-actions",
                    tr!("actions.actions"),
                    PickMode::Actions,
                ))
                .child(mode_button(
                    "onboard-mode-key",
                    tr!("device.onboard_mode_key"),
                    PickMode::Key,
                ))
                .child(mode_button(
                    "onboard-mode-macro",
                    tr!("device.onboard_mode_macro"),
                    PickMode::Macro,
                )),
        )
        .child(body)
}

/// Every action the mouse runs itself, grouped, the current one checked; a
/// click binds it.
fn action_list(
    key: &DeviceKey,
    slot: u8,
    current: &OnboardBinding,
    reachable: bool,
    pal: Palette,
) -> impl IntoElement {
    let mut index = 0usize;
    v_flex().children(ACTION_GROUPS.iter().map(|(heading, actions)| {
        v_flex()
            .child(editor_section(tr!(*heading), pal))
            .children(actions.iter().map(|action| {
                index += 1;
                let selected = *current == OnboardBinding::Action(action.clone());
                let label = action_label(action);
                let key = key.clone();
                let pick = action.clone();
                let debug = format!("onboard-action-{action:?}");
                MenuRow::new(("onboard-action", index))
                    .debug_selector(move || debug.clone())
                    .selected(selected)
                    .role(Role::MenuItem)
                    .aria_label(label.clone())
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .path(binding_icon(&OnboardBinding::Action(action.clone())))
                                    .size_4()
                                    .flex_none()
                                    .text_color(pal.text_muted),
                            )
                            .child(div().child(label)),
                    )
                    .when(selected, |row| {
                        row.child(
                            Icon::new(IconName::Check)
                                .size_3()
                                .text_color(rgb(ACCENT_BLUE)),
                        )
                    })
                    .when(reachable && !selected, |row| {
                        row.on_click(move |_, _, cx| {
                            let pick = pick.clone();
                            AppState::apply(cx, |state| state.set_onboard_button(&key, slot, pick));
                        })
                    })
            }))
    }))
}

/// A key to send: modifiers to hold, the key, what that makes, and Set Key.
fn key_picker(
    drawing: &OnboardDrawing,
    target: OnboardTarget,
    picker: &PickerState,
    chosen_key: Option<HidKey>,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    let toggles = h_flex()
        .gap_1()
        .flex_wrap()
        .children(MODIFIER_TOGGLES.map(|(flag, name)| {
            let view = view.clone();
            control_button(SharedString::new_static(name))
                .label(name)
                .when(picker.modifiers.contains(flag), ButtonVariants::primary)
                .on_click(move |_, _, cx| {
                    view.update(cx, |view, cx| {
                        view.toggle_modifier(flag);
                        cx.notify();
                    });
                })
        }));
    let caption = match picked_chord(picker.modifiers, chosen_key) {
        Some(chord) => SharedString::from(chord.to_string()),
        None if picker.key_missing => tr!("device.onboard_key_choose"),
        None => tr!("device.onboard_key_help"),
    };
    let key = drawing.key.clone();
    let slot = target.slot;
    let set_view = view.clone();
    v_flex()
        .gap_3()
        .child(
            v_flex()
                .gap_1()
                .child(editor_section(tr!("device.onboard_key_hold"), pal))
                .child(toggles),
        )
        .child(
            v_flex()
                .gap_1()
                .child(editor_section(tr!("device.onboard_key_press"), pal))
                .child(control_select(&picker.keys).placeholder(tr!("device.onboard_key_search"))),
        )
        .child(
            div()
                .debug_selector(|| "onboard-key-caption".into())
                .text_caption()
                .text_color(pal.text_muted)
                .child(caption),
        )
        .child(
            control_button("onboard-key-set")
                .primary()
                .w_full()
                .label(tr!("device.onboard_key_set"))
                .disabled(!drawing.reachable)
                .on_click(move |_, _, cx| {
                    let chord = set_view.update(cx, |view, cx| {
                        let chord = view.picked_chord(cx);
                        cx.notify();
                        chord
                    });
                    if let Some(chord) = chord {
                        AppState::apply(cx, |state| {
                            state.set_onboard_button(&key, slot, OnboardAction::Key(chord))
                        });
                    }
                }),
        )
}

/// Why a click button is locked, with a way back to its factory click if
/// the profile lost it.
fn locked_control(
    drawing: &OnboardDrawing,
    target: OnboardTarget,
    button: &OnboardButton,
    pal: Palette,
) -> impl IntoElement {
    let slot = target.slot;
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
