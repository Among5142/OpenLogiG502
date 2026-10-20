//! Drawings of the onboard-profile mice the view knows, authored in the
//! [`MOUSE_MODEL_SIZE`] canvas like the synthetic silhouette: the body
//! outline, the seams between its shells, the button caps, and where each
//! programmable button sits.
//!
//! A button is placed by the role its factory profile gives it, not by its
//! slot: the factory binding is what names the physical button (the G502 X's
//! slot 4 is the DPI-shift button because its ROM profile binds DPI Shift
//! there), and it stays put however the active profile rebinds the slot.
//! No Logi Options+ depot covers these mice, so there is no render or marker
//! metadata to take instead.

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Window, point, px};
use openlogi_core::device::DeviceModelInfo;
use openlogi_core::hid::{
    OnboardAction, OnboardBinding, OnboardButtons, OnboardFunction, OnboardMouseButton,
};

use super::super::hotspots::{ControlTarget, Hotspot, Navigation};

/// A physical button, named by what its factory profile makes it do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Role {
    LeftClick,
    RightClick,
    MiddleClick,
    TiltLeft,
    TiltRight,
    Back,
    Forward,
    DpiUp,
    DpiDown,
    DpiShift,
    ProfileCycle,
}

impl Role {
    /// The role a factory binding gives its button; `None` for a binding no
    /// drawing places.
    fn from_factory(factory: &OnboardBinding) -> Option<Self> {
        let OnboardBinding::Action(action) = factory else {
            return None;
        };
        Some(match action {
            OnboardAction::Mouse(OnboardMouseButton::Left) => Self::LeftClick,
            OnboardAction::Mouse(OnboardMouseButton::Right) => Self::RightClick,
            OnboardAction::Mouse(OnboardMouseButton::Middle) => Self::MiddleClick,
            OnboardAction::Mouse(OnboardMouseButton::Back) => Self::Back,
            OnboardAction::Mouse(OnboardMouseButton::Forward) => Self::Forward,
            OnboardAction::Function(OnboardFunction::TiltLeft) => Self::TiltLeft,
            OnboardAction::Function(OnboardFunction::TiltRight) => Self::TiltRight,
            OnboardAction::Function(OnboardFunction::NextDpi) => Self::DpiUp,
            OnboardAction::Function(OnboardFunction::PreviousDpi) => Self::DpiDown,
            OnboardAction::Function(OnboardFunction::ShiftDpi) => Self::DpiShift,
            OnboardAction::Function(OnboardFunction::CycleProfile) => Self::ProfileCycle,
            _ => return None,
        })
    }
}

/// One of the diagram's targets: the slot a write goes to, and the physical
/// button it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OnboardTarget {
    /// Zero-based button slot in the active profile.
    pub(crate) slot: u8,
    pub(crate) role: Role,
}

impl ControlTarget for OnboardTarget {
    fn navigation(self) -> Option<Navigation> {
        match self.role {
            Role::Back => Some(Navigation::Back),
            Role::Forward => Some(Navigation::Forward),
            _ => None,
        }
    }
}

/// One step of an outline, in model coordinates.
#[derive(Clone, Copy, Debug)]
enum Segment {
    Move(f32, f32),
    /// A cubic Bézier: first control point, second control point, end.
    Cubic([f32; 6]),
    Line(f32, f32),
}

/// What a button cap is painted as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CapTone {
    /// A programmable button.
    Button,
    /// A switch the mouse keeps to itself (the wheel-mode toggle).
    Fixed,
}

/// A button cap drawn on the body: a rounded rectangle.
#[derive(Clone, Copy, Debug)]
pub(super) struct Cap {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
    pub(super) radius: f32,
    pub(super) tone: CapTone,
}

/// Where a role's hotspot sits.
#[derive(Clone, Copy, Debug)]
struct Spot {
    role: Role,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// A drawing of one mouse model.
#[derive(Debug)]
pub(crate) struct Shape {
    /// PIDs the model reports on any transport.
    product_ids: &'static [u16],
    /// The body, one closed path.
    outline: &'static [Segment],
    /// The seams between the body's shells, each an open path.
    seams: &'static [&'static [Segment]],
    pub(super) caps: &'static [Cap],
    spots: &'static [Spot],
    /// The label printed on each button, where the manual gives one.
    printed: &'static [(Role, &'static str)],
}

/// The drawing for a model, if the view has one.
#[must_use]
pub(crate) fn shape_for(model: &DeviceModelInfo) -> Option<&'static Shape> {
    SHAPES.iter().copied().find(|shape| {
        model
            .model_ids
            .iter()
            .any(|pid| *pid != 0 && shape.product_ids.contains(pid))
    })
}

impl Shape {
    /// One hotspot per button of `buttons`, in model coordinates; `None`
    /// when the drawing does not fit them — a button whose factory role it
    /// does not place, or two buttons with one role — so the caller shows
    /// the list instead of pointing at the wrong button.
    #[must_use]
    pub(crate) fn hotspots(&self, buttons: &OnboardButtons) -> Option<Vec<Hotspot<OnboardTarget>>> {
        let mut hotspots: Vec<Hotspot<OnboardTarget>> = Vec::with_capacity(buttons.buttons.len());
        for (slot, button) in buttons.buttons.iter().enumerate() {
            let role = Role::from_factory(button.factory.as_ref()?)?;
            if hotspots.iter().any(|hotspot| hotspot.id.role == role) {
                return None;
            }
            let spot = self.spots.iter().find(|spot| spot.role == role)?;
            hotspots.push(Hotspot {
                id: OnboardTarget {
                    slot: u8::try_from(slot).ok()?,
                    role,
                },
                x: spot.x,
                y: spot.y,
                w: spot.w,
                h: spot.h,
            });
        }
        Some(hotspots)
    }

    /// The label printed on `role`'s button ("G6"), if it has one.
    #[must_use]
    pub(crate) fn printed_name(&self, role: Role) -> Option<&'static str> {
        self.printed
            .iter()
            .find_map(|(printed, name)| (*printed == role).then_some(*name))
    }

    /// Paint the body and its seams into `bounds`, the drawing scaled by
    /// `scale` from model coordinates.
    pub(super) fn paint(
        &self,
        bounds: Bounds<Pixels>,
        scale: f32,
        colors: ShapeColors,
        window: &mut Window,
    ) {
        let body = |builder: PathBuilder| trace(builder, self.outline, bounds, scale);
        if let Ok(path) = body(PathBuilder::fill()).build() {
            window.paint_path(path, colors.body);
        }
        if let Ok(path) = body(PathBuilder::stroke(px(1.5))).build() {
            window.paint_path(path, colors.edge);
        }
        for seam in self.seams {
            if let Ok(path) = trace(PathBuilder::stroke(px(1.)), seam, bounds, scale).build() {
                window.paint_path(path, colors.seam);
            }
        }
    }
}

/// The colours [`Shape::paint`] draws with, from the palette.
#[derive(Clone, Copy, Debug)]
pub(super) struct ShapeColors {
    pub(super) body: Hsla,
    pub(super) edge: Hsla,
    pub(super) seam: Hsla,
}

/// Add `segments` to `builder`, mapped from model coordinates into `bounds`.
fn trace(
    mut builder: PathBuilder,
    segments: &[Segment],
    bounds: Bounds<Pixels>,
    scale: f32,
) -> PathBuilder {
    let at = |x: f32, y: f32| {
        point(
            bounds.origin.x + px(x * scale),
            bounds.origin.y + px(y * scale),
        )
    };
    for segment in segments {
        match *segment {
            Segment::Move(x, y) => builder.move_to(at(x, y)),
            Segment::Line(x, y) => builder.line_to(at(x, y)),
            Segment::Cubic([ax, ay, bx, by, x, y]) => {
                builder.cubic_bezier_to(at(x, y), at(ax, ay), at(bx, by));
            }
        }
    }
    builder
}

const SHAPES: [&Shape; 1] = [&G502_X];

/// G502 X LIGHTSPEED (wireless `0x409f`, wired `0xc098`), from above with
/// the cable end up. Button positions follow the owner's manual: G7/G8 on the
/// strip left of the main click, G9 behind the wheel-mode switch, and G6
/// ahead of G5 and G4 above the thumb rest.
static G502_X: Shape = Shape {
    product_ids: &[0x409f, 0xc098],
    outline: &[
        Segment::Move(150., 28.),
        Segment::Cubic([185., 18., 265., 18., 300., 30.]),
        Segment::Cubic([335., 45., 352., 110., 352., 190.]),
        Segment::Cubic([352., 280., 340., 360., 335., 420.]),
        Segment::Cubic([328., 495., 290., 545., 225., 548.]),
        Segment::Cubic([170., 550., 135., 525., 120., 485.]),
        Segment::Cubic([95., 470., 62., 455., 52., 420.]),
        Segment::Cubic([44., 390., 56., 360., 82., 340.]),
        Segment::Cubic([96., 300., 98., 250., 100., 200.]),
        Segment::Cubic([102., 120., 112., 50., 150., 28.]),
    ],
    seams: &[
        // Rear edge of the main buttons.
        &[
            Segment::Move(104., 215.),
            Segment::Cubic([170., 240., 280., 242., 350., 222.]),
        ],
        // Between the main buttons, down the wheel channel.
        &[Segment::Move(223., 22.), Segment::Line(223., 222.)],
        // The G7/G8 strip beside the main click.
        &[
            Segment::Move(130., 38.),
            Segment::Cubic([123., 90., 121., 150., 125., 212.]),
        ],
        // The thumb rest.
        &[
            Segment::Move(120., 485.),
            Segment::Cubic([140., 430., 120., 380., 82., 340.]),
        ],
    ],
    caps: &[
        // Wheel.
        cap(210., 68., 26., 100., 12., CapTone::Button),
        // Wheel-mode switch.
        cap(214., 180., 18., 18., 4., CapTone::Fixed),
        // G9.
        cap(212., 208., 22., 22., 5., CapTone::Button),
        // G8, G7.
        cap(112., 72., 16., 44., 6., CapTone::Button),
        cap(112., 126., 16., 44., 6., CapTone::Button),
        // G6, G5, G4.
        cap(90., 172., 18., 44., 6., CapTone::Button),
        cap(88., 228., 16., 48., 6., CapTone::Button),
        cap(87., 282., 16., 48., 6., CapTone::Button),
    ],
    spots: &[
        spot(Role::LeftClick, 155., 115., 40., 50.),
        spot(Role::RightClick, 275., 115., 40., 50.),
        spot(Role::MiddleClick, 210., 100., 26., 40.),
        spot(Role::TiltLeft, 190., 70., 20., 30.),
        spot(Role::TiltRight, 236., 70., 20., 30.),
        spot(Role::DpiUp, 108., 74., 24., 40.),
        spot(Role::DpiDown, 108., 128., 24., 40.),
        spot(Role::ProfileCycle, 211., 207., 24., 24.),
        spot(Role::DpiShift, 86., 172., 26., 44.),
        spot(Role::Forward, 83., 228., 26., 48.),
        spot(Role::Back, 82., 282., 26., 48.),
    ],
    printed: &[
        (Role::LeftClick, "G1"),
        (Role::RightClick, "G2"),
        (Role::MiddleClick, "G3"),
        (Role::Back, "G4"),
        (Role::Forward, "G5"),
        (Role::DpiShift, "G6"),
        (Role::DpiDown, "G7"),
        (Role::DpiUp, "G8"),
        (Role::ProfileCycle, "G9"),
    ],
};

const fn cap(x: f32, y: f32, w: f32, h: f32, radius: f32, tone: CapTone) -> Cap {
    Cap {
        x,
        y,
        w,
        h,
        radius,
        tone,
    }
}

const fn spot(role: Role, x: f32, y: f32, w: f32, h: f32) -> Spot {
    Spot { role, x, y, w, h }
}

#[cfg(test)]
mod tests {
    use openlogi_core::hid::{OnboardButton, OnboardButtons};

    use super::*;
    use crate::features::mouse::hotspots::MOUSE_MODEL_SIZE;

    /// The G502 X's factory roles, in slot order.
    fn g502x_buttons() -> OnboardButtons {
        let factory = [
            OnboardAction::Mouse(OnboardMouseButton::Left),
            OnboardAction::Mouse(OnboardMouseButton::Right),
            OnboardAction::Mouse(OnboardMouseButton::Middle),
            OnboardAction::Mouse(OnboardMouseButton::Back),
            OnboardAction::Function(OnboardFunction::ShiftDpi),
            OnboardAction::Mouse(OnboardMouseButton::Forward),
            OnboardAction::Function(OnboardFunction::TiltLeft),
            OnboardAction::Function(OnboardFunction::TiltRight),
            OnboardAction::Function(OnboardFunction::CycleProfile),
            OnboardAction::Function(OnboardFunction::NextDpi),
            OnboardAction::Function(OnboardFunction::PreviousDpi),
        ];
        OnboardButtons {
            sector: 1,
            buttons: factory
                .into_iter()
                .map(|action| OnboardButton {
                    binding: OnboardBinding::Action(OnboardAction::Disabled),
                    factory: Some(OnboardBinding::Action(action)),
                })
                .collect(),
        }
    }

    fn model(pids: [u16; 3]) -> DeviceModelInfo {
        DeviceModelInfo {
            entity_count: 1,
            serial_number: None,
            unit_id: [0; 4],
            transports: openlogi_core::device::DeviceTransports::default(),
            model_ids: pids,
            extended_model_id: 0,
        }
    }

    #[test]
    fn the_g502x_is_drawn_on_either_connection() {
        assert!(shape_for(&model([0x409f, 0xc098, 0])).is_some());
        assert!(shape_for(&model([0xc098, 0, 0])).is_some());
        assert!(shape_for(&model([0x4093, 0, 0])).is_none());
        assert!(shape_for(&model([0, 0, 0])).is_none());
    }

    #[test]
    fn every_g502x_button_lands_on_its_own_spot_inside_the_canvas() {
        let hotspots = G502_X.hotspots(&g502x_buttons()).expect("the drawing fits");
        assert_eq!(hotspots.len(), 11);
        assert_eq!(
            hotspots[4].id,
            OnboardTarget {
                slot: 4,
                role: Role::DpiShift
            }
        );
        for hotspot in &hotspots {
            assert!(hotspot.x >= 0. && hotspot.x + hotspot.w <= MOUSE_MODEL_SIZE.0);
            assert!(hotspot.y >= 0. && hotspot.y + hotspot.h <= MOUSE_MODEL_SIZE.1);
        }
    }

    #[test]
    fn a_button_the_drawing_cannot_place_falls_back_to_the_list() {
        let mut unknown = g502x_buttons();
        unknown.buttons[8].factory = Some(OnboardBinding::Action(OnboardAction::Function(
            OnboardFunction::BatteryStatus,
        )));
        assert!(G502_X.hotspots(&unknown).is_none());

        let mut unset = g502x_buttons();
        unset.buttons[8].factory = None;
        assert!(G502_X.hotspots(&unset).is_none());

        let mut twice = g502x_buttons();
        twice.buttons[8].factory = twice.buttons[9].factory.clone();
        assert!(G502_X.hotspots(&twice).is_none(), "one role, two buttons");
    }
}
