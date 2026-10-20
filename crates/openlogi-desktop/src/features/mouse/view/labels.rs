//! The binding labels beside the mouse render: what each control card says and how it is placed in the side gutter.

use std::rc::Rc;

use super::super::controls::{BindingLabel, ControlCard, LABEL_W};
use super::super::geometry::LABEL_H;
use super::super::hotspots::MouseControlId;
use super::super::leader_lines::{Label, Side};
use super::super::thumbwheel::ThumbwheelPreset;
use crate::features::binding_editor::{GESTURE_BUTTON_ICON, action_icon_path};
use crate::ui::action::localized_action_label;
use gpui::{Entity, InteractiveElement, ParentElement, Styled, div, px};
use openlogi_core::binding::{Action, ButtonId, default_binding};

use super::{ModelRect, MouseModelView, SIDE_GAP, set_control_hovered};

/// Position a selectable control card at the label's slot in the side gutter.
/// Selection updates the fixed inspector; labels never own editor overlays.
pub(super) fn label_control(
    idx: usize,
    label: Label,
    binding: BindingLabel,
    highlighted: bool,
    model: ModelRect,
    selected: bool,
    view: &Entity<MouseModelView>,
) -> gpui::Div {
    let x = match label.side {
        Side::Left => model.left - SIDE_GAP - LABEL_W,
        Side::Right => model.left + model.width + SIDE_GAP,
    };
    let control = label.id;
    let select_view = view.clone();
    let hover_view = view.clone();
    let trigger = ControlCard {
        id: ("label-trigger", idx).into(),
        name: tr!(control.translation_key()),
        binding,
        highlighted,
        selected,
        value_selector: format!("label-value-row-{control:?}"),
        on_select: Rc::new(move |cx| {
            select_view.update(cx, |this, cx| {
                this.select(control);
                cx.notify();
            });
        }),
        on_hover: Rc::new(move |hovered, cx| {
            set_control_hovered(&hover_view, control, hovered, cx);
        }),
    };
    div()
        .absolute()
        .left(px(x))
        .top(px(label.y - LABEL_H / 2.))
        .w(px(LABEL_W))
        .h(px(LABEL_H))
        .debug_selector(move || format!("label-card-{control:?}"))
        .child(trigger)
}

/// The label card's text and icon for one control.
pub(super) fn binding_label_for_control(
    control: MouseControlId,
    bindings: &std::collections::BTreeMap<ButtonId, Action>,
    gesture_buttons: &[ButtonId],
) -> BindingLabel {
    if control
        .button()
        .is_some_and(|button| gesture_buttons.contains(&button))
    {
        return BindingLabel {
            text: tr!("actions.five_directions"),
            icon: Some(GESTURE_BUTTON_ICON),
        };
    }

    match control {
        MouseControlId::Button(button) => {
            let action = bindings
                .get(&button)
                .cloned()
                .unwrap_or_else(|| default_binding(button));
            BindingLabel {
                text: localized_action_label(&action),
                icon: Some(action_icon_path(&action)),
            }
        }
        MouseControlId::ThumbwheelRotation => {
            let backward = bindings
                .get(&ButtonId::ThumbwheelScrollDown)
                .cloned()
                .unwrap_or_else(|| default_binding(ButtonId::ThumbwheelScrollDown));
            let forward = bindings
                .get(&ButtonId::ThumbwheelScrollUp)
                .cloned()
                .unwrap_or_else(|| default_binding(ButtonId::ThumbwheelScrollUp));
            if let Some(preset) = ThumbwheelPreset::recognize(&backward, &forward) {
                BindingLabel {
                    text: tr!(preset.translation_key()),
                    icon: Some(preset.icon()),
                }
            } else {
                BindingLabel {
                    text: tr!("common.custom"),
                    icon: Some("action-icons/chevrons-right.svg"),
                }
            }
        }
    }
}
