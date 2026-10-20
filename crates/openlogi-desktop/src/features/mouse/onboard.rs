//! The Onboard Buttons tab for a mouse that binds its buttons in an HID++
//! `0x8100` onboard profile: the same diagram the Buttons tab draws — the
//! mouse, a dot on each button, a card per button in the side gutter, and a
//! fixed inspector — keyed by profile slot instead of `ButtonId`.
//!
//! It draws only a mouse it has a [`Shape`] for and whose buttons that shape
//! places; any other onboard mouse gets the [`list`] card. Nothing is stored
//! on this computer: a pick goes through [`AppState::set_onboard_button`].

mod actions;
mod inspector;
pub(crate) mod list;
pub(crate) mod shape;

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, canvas, div, hsla, px,
};
use gpui_component::{
    Disableable as _, IconName,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use openlogi_core::hid::{OnboardButton, OnboardButtons};

use self::actions::{binding_icon, binding_label};
use self::inspector::onboard_inspector;
use self::shape::{CapTone, OnboardTarget, Shape, ShapeColors, shape_for};
use super::controls::{BindingLabel, ControlCard, HotspotDot, LABEL_W, OnHover, OnSelect};
use super::geometry::{LABEL_H, labels_from_hotspots};
use super::hotspots::{Hotspot, MOUSE_MODEL_SIZE};
use super::leader_lines::{Label, Side};
use super::view::{ModelFit, ModelRect, SIDE_GAP, leader_canvas, workspace_layout};
use crate::state::{AppState, DeviceKey, OnboardButtonRefusal, OnboardButtonsLoad, StateEvent};
use crate::ui::theme::{self, Palette, Typography as _};

/// What the drawing needs about the selected mouse.
pub(crate) struct OnboardDrawing {
    key: DeviceKey,
    buttons: Arc<OnboardButtons>,
    shape: &'static Shape,
    /// One per button, in model coordinates.
    hotspots: Vec<Hotspot<OnboardTarget>>,
    reachable: bool,
    refusal: Option<OnboardButtonRefusal>,
}

impl OnboardDrawing {
    /// The selected mouse's drawing, or `None` when the tab shows the list
    /// instead: nothing selected, no onboard profile, its buttons not read
    /// yet, or no drawing that places every one of them.
    pub(crate) fn current(cx: &App) -> Option<Self> {
        let state = AppState::try_read(cx)?;
        if !state.current_onboard_buttons_supported() {
            return None;
        }
        let record = state.current_record()?;
        let shape = shape_for(record.model_info.as_ref()?)?;
        let OnboardButtonsLoad::Ready(buttons) = state.current_onboard_buttons_load() else {
            return None;
        };
        let hotspots = shape.hotspots(&buttons)?;
        Some(Self {
            key: record.device_key(),
            buttons,
            shape,
            hotspots,
            reachable: state.current_onboard_buttons_reachable(),
            refusal: state.current_onboard_button_refusal(),
        })
    }

    fn button(&self, target: OnboardTarget) -> Option<&OnboardButton> {
        self.buttons.buttons.get(usize::from(target.slot))
    }
}

/// Whether a button gets a card in the gutter. A click button is locked to
/// its factory click, so it gets one only once its profile lost that click
/// and it needs putting back; its dot stays on the drawing either way.
fn shows_card(button: &OnboardButton) -> bool {
    !button.locked() || button.factory.as_ref() != Some(&button.binding)
}

/// What a button is called: its factory role ("DPI Shift"), which names the
/// physical button.
fn button_name(button: &OnboardButton) -> gpui::SharedString {
    button
        .factory
        .as_ref()
        .map_or_else(|| tr!("device.onboard_disabled"), binding_label)
}

/// The onboard-profile mouse diagram with its inspector.
pub struct OnboardModelView {
    focus_handle: FocusHandle,
    device: Option<DeviceKey>,
    hovered: Option<OnboardTarget>,
    selected: Option<OnboardTarget>,
    _state_obs: Subscription,
}

impl OnboardModelView {
    /// Create the onboard diagram.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state_obs = AppState::repaint_on(cx, |event| {
            matches!(event, StateEvent::OnboardButtonsChanged(_))
        });
        Self {
            focus_handle: cx.focus_handle(),
            device: None,
            hovered: None,
            selected: None,
            _state_obs: state_obs,
        }
    }

    fn reset_for_device(&mut self, device: &DeviceKey) {
        if self.device.as_ref() == Some(device) {
            return;
        }
        self.device = Some(device.clone());
        self.hovered = None;
        self.selected = None;
    }
}

impl Focusable for OnboardModelView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Select `target` when its dot or card is clicked.
fn on_select(view: &Entity<OnboardModelView>, target: OnboardTarget) -> OnSelect {
    let view = view.clone();
    Rc::new(move |cx| {
        view.update(cx, |this, cx| {
            this.selected = Some(target);
            cx.notify();
        });
    })
}

/// Track the pointer entering or leaving `target`'s dot or card.
fn on_hover(view: &Entity<OnboardModelView>, target: OnboardTarget) -> OnHover {
    let view = view.clone();
    Rc::new(move |hovered, cx| {
        view.update(cx, |this, cx| {
            if hovered {
                this.hovered = Some(target);
            } else if this.hovered == Some(target) {
                this.hovered = None;
            }
            cx.notify();
        });
    })
}

impl Render for OnboardModelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(drawing) = OnboardDrawing::current(cx) else {
            return div().into_any_element();
        };
        self.reset_for_device(&drawing.key);
        let pal = theme::palette(cx);
        let view = cx.entity();

        let viewport = window.viewport_size();
        let fit = ModelFit::new(f32::from(viewport.width), f32::from(viewport.height), true);
        let scale = fit.drawing_scale();
        let model = ModelRect {
            left: fit.left_gutter,
            width: MOUSE_MODEL_SIZE.0 * scale,
            height: MOUSE_MODEL_SIZE.1 * scale,
        };
        let hotspots: Vec<Hotspot<OnboardTarget>> = drawing
            .hotspots
            .iter()
            .map(|hotspot| Hotspot {
                x: hotspot.x * scale,
                y: hotspot.y * scale,
                w: hotspot.w * scale,
                h: hotspot.h * scale,
                ..*hotspot
            })
            .collect();
        let carded: Vec<Hotspot<OnboardTarget>> = hotspots
            .iter()
            .filter(|hotspot| drawing.button(hotspot.id).is_some_and(shows_card))
            .copied()
            .collect();
        let labels = if fit.has_labels {
            labels_from_hotspots(&carded, model.height, fit.distribution)
        } else {
            Vec::new()
        };
        let highlight = self.hovered.or(self.selected);

        let cards = labels.iter().enumerate().filter_map(|(idx, label)| {
            let button = drawing.button(label.id)?;
            Some(card(
                idx,
                *label,
                button,
                highlight == Some(label.id),
                self.selected == Some(label.id),
                model,
                &view,
            ))
        });
        let dots = hotspots.iter().enumerate().filter_map(|(idx, hotspot)| {
            let button = drawing.button(hotspot.id)?;
            Some(dot(
                idx,
                *hotspot,
                button,
                self.hovered == Some(hotspot.id),
                self.selected == Some(hotspot.id),
                &view,
            ))
        });
        let canvas_area = div()
            .relative()
            .w(px(fit.left_gutter + model.width + fit.right_gutter))
            .h(px(model.height))
            .child(body(drawing.shape, model, scale, pal))
            .child(leader_canvas(
                hotspots.clone(),
                labels.clone(),
                highlight,
                model.left,
                model.width,
            ))
            .children(cards)
            .child(
                div()
                    .absolute()
                    .left(px(model.left))
                    .top(px(0.))
                    .w(px(model.width))
                    .h(px(model.height))
                    .children(dots),
            );

        let inspector = onboard_inspector(&drawing, self.selected, pal);
        workspace_layout(
            canvas_area,
            Some(profile_bar(&drawing, pal)),
            inspector,
            &self.focus_handle,
        )
        .into_any_element()
    }
}

/// The mouse itself: the outline and seams painted, the caps laid on top.
fn body(shape: &'static Shape, model: ModelRect, scale: f32, pal: Palette) -> gpui::Div {
    let colors = ShapeColors {
        body: pal.panel,
        edge: pal.text_muted,
        seam: pal.border,
    };
    div()
        .absolute()
        .left(px(model.left))
        .top(px(0.))
        .w(px(model.width))
        .h(px(model.height))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| shape.paint(bounds, scale, colors, window),
            )
            .size_full(),
        )
        .children(shape.caps.iter().map(move |cap| {
            div()
                .absolute()
                .left(px(cap.x * scale))
                .top(px(cap.y * scale))
                .w(px(cap.w * scale))
                .h(px(cap.h * scale))
                .rounded(px(cap.radius * scale))
                .bg(match cap.tone {
                    // Illustration tones, matching the synthetic silhouette's
                    // side buttons rather than tracking the palette.
                    CapTone::Button => hsla(0., 0., 0.25, 1.0),
                    CapTone::Fixed => hsla(0., 0., 0.42, 1.0),
                })
        }))
}

/// A button's card in the side gutter.
fn card(
    idx: usize,
    label: Label<OnboardTarget>,
    button: &OnboardButton,
    highlighted: bool,
    selected: bool,
    model: ModelRect,
    view: &Entity<OnboardModelView>,
) -> gpui::Div {
    let x = match label.side {
        Side::Left => model.left - SIDE_GAP - LABEL_W,
        Side::Right => model.left + model.width + SIDE_GAP,
    };
    let slot = label.id.slot;
    div()
        .absolute()
        .left(px(x))
        .top(px(label.y - LABEL_H / 2.))
        .w(px(LABEL_W))
        .h(px(LABEL_H))
        .debug_selector(move || format!("onboard-card-{slot}"))
        .child(ControlCard {
            id: ("onboard-card", idx).into(),
            name: button_name(button),
            binding: BindingLabel {
                text: binding_label(&button.binding),
                icon: Some(binding_icon(&button.binding)),
            },
            highlighted,
            selected,
            value_selector: format!("onboard-card-value-{slot}"),
            on_select: on_select(view, label.id),
            on_hover: on_hover(view, label.id),
        })
}

/// A button's dot on the drawing.
fn dot(
    idx: usize,
    hotspot: Hotspot<OnboardTarget>,
    button: &OnboardButton,
    hovered: bool,
    selected: bool,
    view: &Entity<OnboardModelView>,
) -> gpui::Div {
    div()
        .absolute()
        .left(px(hotspot.x))
        .top(px(hotspot.y))
        .w(px(hotspot.w))
        .h(px(hotspot.h))
        .child(HotspotDot {
            id: ("onboard-dot", idx).into(),
            name: button_name(button),
            w: hotspot.w,
            h: hotspot.h,
            highlighted: hovered,
            selected,
            on_select: on_select(view, hotspot.id),
            on_hover: on_hover(view, hotspot.id),
        })
}

/// Above the drawing: which onboard profile is being edited, where the
/// bindings live, and a way to read them again.
fn profile_bar(drawing: &OnboardDrawing, pal: Palette) -> gpui::Div {
    let key = drawing.key.clone();
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .px_4()
        .pt_3()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_body().text_color(pal.text_primary).child(tr!(
                    "device.onboard_profile_number",
                    profile => drawing.buttons.sector
                )))
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
                .disabled(!drawing.reachable)
                .on_click(move |_event, _window, cx| {
                    AppState::apply(cx, |state| state.refresh_onboard_buttons(&key));
                }),
        )
}

#[cfg(test)]
mod tests {
    use openlogi_core::hid::{OnboardAction, OnboardBinding, OnboardMouseButton};

    use super::*;

    fn button(binding: OnboardAction, factory: OnboardAction) -> OnboardButton {
        OnboardButton {
            binding: OnboardBinding::Action(binding),
            factory: Some(OnboardBinding::Action(factory)),
        }
    }

    #[test]
    fn a_click_gets_a_card_only_once_it_needs_restoring() {
        let left = OnboardAction::Mouse(OnboardMouseButton::Left);
        let back = OnboardAction::Mouse(OnboardMouseButton::Back);
        assert!(!shows_card(&button(left.clone(), left.clone())));
        assert!(shows_card(&button(OnboardAction::Disabled, left)));
        assert!(shows_card(&button(back.clone(), back)));
    }
}
