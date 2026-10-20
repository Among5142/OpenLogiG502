//! The two selectable pieces every mouse diagram draws for a control: the
//! dot on the render and the card in the side gutter. The Options+-style
//! button view and the onboard view both place them; what a click selects is
//! the caller's.

use std::rc::Rc;

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, hsla, prelude::FluentBuilder as _, px,
    rgb, svg,
};
use gpui_base::Button as BaseButton;
use gpui_component::{Icon, IconName, h_flex};

use super::geometry::LABEL_H;
use crate::ui::theme::{self, ACCENT_BLUE, Typography as _};

/// Width of a side-label card.
pub(crate) const LABEL_W: f32 = 156.;

/// Diameter of the dot drawn at a hotspot.
const HOTSPOT_DOT: f32 = 12.;

/// Called when the control is clicked.
pub(crate) type OnSelect = Rc<dyn Fn(&mut App)>;
/// Called when the pointer enters (`true`) or leaves (`false`) the control.
pub(crate) type OnHover = Rc<dyn Fn(bool, &mut App)>;

/// What a control's card shows as its value: the action, with its icon.
pub(crate) struct BindingLabel {
    pub(crate) text: SharedString,
    /// Vendored action-icon asset path for the card's leading glyph; `None`
    /// draws the text alone.
    pub(crate) icon: Option<&'static str>,
}

impl BindingLabel {
    /// The card's value row: the leading action icon, the binding text, and
    /// the trailing chevron, all tinted with `color`.
    fn row(self, color: Hsla, pal: theme::Palette) -> gpui::Div {
        h_flex()
            .items_center()
            .gap_2()
            // Leading action icon (same glyph as the picker rows), tinted with
            // the value so it tracks the default / set / highlighted state.
            .when_some(self.icon, |row, path| {
                row.child(svg().path(path).size_4().flex_none().text_color(color))
            })
            .child(
                // Shrink + ellipsis so a long action name (e.g. "Mission
                // Control") doesn't push the chevron out of the fixed card.
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_body()
                    .text_color(color)
                    .child(self.text),
            )
            .child(
                Icon::new(IconName::ChevronRight)
                    .size_3()
                    .text_color(pal.text_muted),
            )
    }
}

/// A side-label card: the control's name above what it does now.
#[derive(IntoElement)]
pub(crate) struct ControlCard {
    pub(crate) id: ElementId,
    /// The control's name — the card's caption and its accessible name.
    pub(crate) name: SharedString,
    pub(crate) binding: BindingLabel,
    /// Hovered, or what the mouse is pressing now.
    pub(crate) highlighted: bool,
    pub(crate) selected: bool,
    /// Test selector for the value row.
    pub(crate) value_selector: String,
    pub(crate) on_select: OnSelect,
    pub(crate) on_hover: OnHover,
}

impl RenderOnce for ControlCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let highlighted = self.highlighted || self.selected;
        let selected = self.selected;
        let pal = theme::palette(cx);
        let binding_color = if highlighted {
            rgb(ACCENT_BLUE).into()
        } else {
            pal.text_primary
        };
        // Always show the action the button actually performs. Default and
        // customised bindings use the same neutral value colour; only the
        // actively highlighted control takes the accent.
        let binding_description = self.binding.text.clone();
        let value_selector = self.value_selector;
        let on_select = self.on_select;
        let on_hover = self.on_hover;
        BaseButton::new(self.id)
            .selected(selected)
            .accessibility_label(tr!("actions.bind_control", name => self.name.clone()))
            .aria_description(binding_description)
            .aria_selected(selected)
            .flex()
            .flex_col()
            .items_stretch()
            .w(px(LABEL_W))
            .h(px(LABEL_H))
            .px_3()
            .justify_center()
            .gap_0p5()
            .rounded(pal.control_radius)
            .border_1()
            .border_color(if highlighted {
                rgb(ACCENT_BLUE).into()
            } else {
                pal.border
            })
            .bg(if highlighted {
                theme::accent_tint()
            } else {
                pal.control
            })
            .cursor_pointer()
            .hover(move |s| {
                s.bg(if highlighted {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
            })
            .focus_visible(move |s| {
                s.bg(if highlighted {
                    theme::accent_tint_hover()
                } else {
                    pal.control_hover
                })
                .border_color(rgb(ACCENT_BLUE))
            })
            // Button name — the caption (xs / muted), the same size as the
            // inspector title and category headers it shares the binding flow with.
            .child(
                div()
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(self.name),
            )
            // Current binding — the value (sm), the same size as the action rows
            // it edits.
            .child(
                self.binding
                    .row(binding_color, pal)
                    .debug_selector(move || value_selector.clone()),
            )
            .on_click(move |_event, _window, cx| on_select(cx))
            .on_hover(move |hovered, _window, cx| on_hover(*hovered, cx))
    }
}

/// The dot drawn at a hotspot, filling a `w` × `h` hit target.
#[derive(IntoElement)]
pub(crate) struct HotspotDot {
    pub(crate) id: ElementId,
    /// The control's name, for its accessible label.
    pub(crate) name: SharedString,
    pub(crate) w: f32,
    pub(crate) h: f32,
    /// Hovered, or what the mouse is pressing now.
    pub(crate) highlighted: bool,
    pub(crate) selected: bool,
    pub(crate) on_select: OnSelect,
    pub(crate) on_hover: OnHover,
}

impl RenderOnce for HotspotDot {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let highlighted = self.highlighted || self.selected;
        let selected = self.selected;
        let on_select = self.on_select;
        let on_hover = self.on_hover;
        BaseButton::new(self.id)
            .selected(selected)
            .accessibility_label(tr!("actions.bind_control", name => self.name))
            .aria_selected(selected)
            .flex()
            .items_center()
            .justify_center()
            .w(px(self.w))
            .h(px(self.h))
            .child(
                div()
                    .w(px(HOTSPOT_DOT))
                    .h(px(HOTSPOT_DOT))
                    .rounded_full()
                    .border_1()
                    .border_color(if highlighted {
                        gpui::Hsla::from(rgb(ACCENT_BLUE))
                    } else {
                        hsla(0., 0., 0.95, 0.85)
                    })
                    .bg(if highlighted {
                        gpui::Hsla::from(rgb(ACCENT_BLUE))
                    } else {
                        hsla(0., 0., 0.18, 0.85)
                    }),
            )
            .focus_visible(|style| {
                style
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(ACCENT_BLUE))
            })
            .on_click(move |_event, _window, cx| on_select(cx))
            .on_hover(move |hovered, _window, cx| on_hover(*hovered, cx))
    }
}
