//! The Macro picker: a macro the mouse stores and runs itself, built step by
//! step or recorded, then saved onto the button.
//!
//! The draft lives here until Save sends it through
//! [`AppState::set_onboard_button`]; the mouse's read-back then becomes what
//! the button runs. A step is a keystroke or mouse button held for a time,
//! a wait, or — from a recording where inputs overlap — a lone press or
//! release.
//!
//! Recording captures what reaches the record pad while it has focus: keys,
//! the clicks made on it, and the modifiers. Input to other windows is not
//! seen, and the OS keeps a few combinations (the Windows key, Alt+Tab) to
//! itself.

use std::time::Instant;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyUpEvent, KeystrokeEvent, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseUpEvent, NavigationDirection, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, rgb, svg,
};
use gpui_component::{
    Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::InputState,
    v_flex,
};
use openlogi_core::hid::{
    HidKey, HidKeyChord, MacroEvent, MacroInput, MacroStep, OnboardAction, OnboardBinding,
    OnboardMacro, OnboardMouseButton,
};

use tracing::debug;

use super::actions::{MODIFIER_TOGGLES, action_label, picked_chord};
use super::inspector::Picking;
use super::shape::OnboardTarget;
use super::{OnboardDrawing, OnboardModelView, PickerState};
use crate::features::binding_editor::editor_section;
use crate::state::AppState;
use crate::ui::components::{control_button, control_input, control_select};
use crate::ui::theme::{ACCENT_BLUE, Palette, Typography as _};

/// What the Add step form puts together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AddKind {
    Keystroke,
    MouseButton,
    Delay,
}

/// The macro being edited for the selected button.
pub(super) struct MacroDraft {
    pub(super) steps: Vec<MacroStep>,
    /// The Add step form, while open.
    adding: Option<AddKind>,
    /// The mouse button the Add step form presses.
    add_mouse: OnboardMouseButton,
    /// The step whose time the editor under it changes.
    editing: Option<usize>,
    recording: Option<Recording>,
    /// A time field held something other than 0–65535.
    invalid_ms: bool,
    hold_ms: Entity<InputState>,
    delay_ms: Entity<InputState>,
    edit_ms: Entity<InputState>,
    /// The record pad, which takes the key releases and clicks while
    /// recording.
    pad: FocusHandle,
    /// Takes every key press in the window while recording, before any key
    /// binding can act on it (Tab would move focus, Enter press a button).
    intercept: Option<Subscription>,
}

impl MacroDraft {
    pub(super) fn new(window: &mut Window, cx: &mut Context<OnboardModelView>) -> Self {
        let ms_input = |value: &str, window: &mut Window, cx: &mut Context<OnboardModelView>| {
            let value = value.to_string();
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder("ms");
                input.set_value(value, window, cx);
                input
            })
        };
        Self {
            steps: Vec::new(),
            adding: None,
            add_mouse: OnboardMouseButton::Left,
            editing: None,
            recording: None,
            invalid_ms: false,
            hold_ms: ms_input("50", window, cx),
            delay_ms: ms_input("100", window, cx),
            edit_ms: ms_input("", window, cx),
            pad: cx.focus_handle(),
            intercept: None,
        }
    }

    /// Start from what the button runs now: its macro, or nothing.
    pub(super) fn load(&mut self, binding: &OnboardBinding) {
        self.steps = match binding {
            OnboardBinding::Action(OnboardAction::Macro(program)) => program.steps.clone(),
            _ => Vec::new(),
        };
        self.adding = None;
        self.editing = None;
        self.recording = None;
        self.intercept = None;
        self.invalid_ms = false;
    }

    /// Whether the draft differs from what the button runs now, as the
    /// mouse last read back.
    fn unsaved(&self, current: &OnboardBinding) -> bool {
        match current {
            OnboardBinding::Action(OnboardAction::Macro(program)) => program.steps != self.steps,
            _ => !self.steps.is_empty(),
        }
    }
}

/// A recording in progress: the events so far, timed from one to the next.
#[derive(Debug, Default)]
pub(super) struct Recording {
    events: Vec<MacroEvent>,
    last: Option<Instant>,
    /// Inputs pressed during the recording and not yet released.
    held: Vec<MacroInput>,
    modifiers: Modifiers,
}

/// Reads one modifier's state from [`Modifiers`].
type ModifierFlag = fn(&Modifiers) -> bool;

/// The left-hand modifier keys a recording names, with the flag each reads
/// from [`Modifiers`].
const RECORDED_MODIFIERS: [(u8, ModifierFlag); 4] = [
    (0xe0, |m| m.control),
    (0xe1, |m| m.shift),
    (0xe2, |m| m.alt),
    (0xe3, |m| m.platform),
];

impl Recording {
    fn press(&mut self, input: MacroInput, now: Instant) {
        if self.held.contains(&input) {
            return;
        }
        self.held.push(input);
        self.push(MacroEvent::Press(input), now);
    }

    /// Release `input`; one pressed before the recording began is ignored,
    /// so the macro never releases what it did not press.
    fn release(&mut self, input: MacroInput, now: Instant) {
        let Some(at) = self.held.iter().position(|held| *held == input) else {
            return;
        };
        self.held.remove(at);
        self.push(MacroEvent::Release(input), now);
    }

    fn modifiers_changed(&mut self, modifiers: Modifiers, now: Instant) {
        for (usage, flag) in RECORDED_MODIFIERS {
            let Ok(key) = HidKey::try_from(usage) else {
                continue;
            };
            let input = MacroInput::Key(HidKeyChord::key(key));
            match (flag(&self.modifiers), flag(&modifiers)) {
                (false, true) => self.press(input, now),
                (true, false) => self.release(input, now),
                _ => {}
            }
        }
        self.modifiers = modifiers;
    }

    fn push(&mut self, event: MacroEvent, now: Instant) {
        // A wait under a millisecond is no wait the mouse can run.
        if let Some(last) = self.last {
            let waited = now.saturating_duration_since(last).as_millis();
            if waited > 0 {
                self.events
                    .push(MacroEvent::Delay(u16::try_from(waited).unwrap_or(u16::MAX)));
            }
        }
        self.events.push(event);
        self.last = Some(now);
    }

    /// The recorded steps, every input still held released at the end so
    /// the macro leaves nothing stuck down.
    fn finish(mut self, now: Instant) -> Vec<MacroStep> {
        for input in self.held.clone().into_iter().rev() {
            self.release(input, now);
        }
        OnboardMacro::from_events(&self.events).steps
    }
}

/// The key a key event names, by GPUI's key name, or `None` for one with no
/// standard keyboard usage.
fn recorded_key(name: &str) -> Option<HidKey> {
    let name = match name {
        "pageup" => "Page Up",
        "pagedown" => "Page Down",
        // "Menu" is also the name of usage 0x76, a key no keyboard here has.
        "menu" => "Application",
        other => other,
    };
    HidKey::from_name(name).filter(|key| HidKey::standard().any(|standard| standard == *key))
}

/// The standard mouse button a GPUI button is.
fn recorded_button(button: MouseButton) -> OnboardMouseButton {
    match button {
        MouseButton::Left => OnboardMouseButton::Left,
        MouseButton::Right => OnboardMouseButton::Right,
        MouseButton::Middle => OnboardMouseButton::Middle,
        MouseButton::Navigate(NavigationDirection::Back) => OnboardMouseButton::Back,
        MouseButton::Navigate(NavigationDirection::Forward) => OnboardMouseButton::Forward,
    }
}

/// Parse a time field: whole milliseconds, 0–65535.
fn read_ms(input: &Entity<InputState>, cx: &App) -> Option<u16> {
    input.read(cx).value().trim().parse().ok()
}

impl OnboardModelView {
    fn macro_start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.macro_draft.recording = Some(Recording::default());
        self.macro_draft.editing = None;
        self.macro_draft.adding = None;
        let view = cx.entity().downgrade();
        self.macro_draft.intercept = Some(cx.intercept_keystrokes(
            move |event: &KeystrokeEvent, _, cx| {
                let Some(view) = view.upgrade() else {
                    return;
                };
                let pressed = event.keystroke.modifiers;
                let key = recorded_key(&event.keystroke.key);
                debug!(key = %event.keystroke.key, recorded = ?key, "macro recording: key down");
                cx.stop_propagation();
                update(&view, cx, |view, _| {
                    view.macro_record(|recording, now| {
                        // The modifiers held with the key, in case their own
                        // change did not reach the pad first.
                        recording.modifiers_changed(pressed, now);
                        if let Some(key) = key {
                            recording.press(MacroInput::Key(HidKeyChord::key(key)), now);
                        }
                    });
                });
            },
        ));
        // The pad is drawn by the next frame; focus it then, after the
        // Record button's own click handling.
        let pad = self.macro_draft.pad.clone();
        window.on_next_frame(move |window, cx| window.focus(&pad, cx));
    }

    fn macro_stop_recording(&mut self) {
        self.macro_draft.intercept = None;
        if let Some(recording) = self.macro_draft.recording.take() {
            self.macro_draft.steps = recording.finish(Instant::now());
        }
    }

    fn macro_record(&mut self, edit: impl FnOnce(&mut Recording, Instant)) {
        if let Some(recording) = self.macro_draft.recording.as_mut() {
            edit(recording, Instant::now());
        }
    }

    fn macro_add(&mut self, cx: &mut Context<Self>) {
        let draft = &mut self.macro_draft;
        let step = match draft.adding {
            None => return,
            Some(AddKind::Delay) => read_ms(&draft.delay_ms, cx).map(MacroStep::Delay),
            Some(AddKind::MouseButton) => {
                read_ms(&draft.hold_ms, cx).map(|hold_ms| MacroStep::Stroke {
                    input: MacroInput::Mouse(draft.add_mouse),
                    hold_ms,
                })
            }
            Some(AddKind::Keystroke) => {
                let Some(hold_ms) = read_ms(&draft.hold_ms, cx) else {
                    draft.invalid_ms = true;
                    return;
                };
                let key = self.picker.keys.read(cx).selected_value().copied();
                let Some(chord) = picked_chord(self.picker.modifiers, key) else {
                    self.picker.key_missing = true;
                    return;
                };
                Some(MacroStep::Stroke {
                    input: MacroInput::Key(chord),
                    hold_ms,
                })
            }
        };
        let draft = &mut self.macro_draft;
        match step {
            Some(step) => {
                draft.steps.push(step);
                draft.invalid_ms = false;
            }
            None => draft.invalid_ms = true,
        }
    }

    fn macro_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let draft = &mut self.macro_draft;
        let Some(ms) = draft.steps.get(index).copied().and_then(step_ms) else {
            return;
        };
        draft.editing = Some(index);
        draft.invalid_ms = false;
        draft
            .edit_ms
            .update(cx, |input, cx| input.set_value(ms.to_string(), window, cx));
    }

    fn macro_apply_edit(&mut self, cx: &mut Context<Self>) {
        let draft = &mut self.macro_draft;
        let Some(index) = draft.editing else {
            return;
        };
        let Some(ms) = read_ms(&draft.edit_ms, cx) else {
            draft.invalid_ms = true;
            return;
        };
        match draft.steps.get_mut(index) {
            Some(MacroStep::Stroke { hold_ms, .. }) => *hold_ms = ms,
            Some(MacroStep::Delay(delay)) => *delay = ms,
            _ => {}
        }
        draft.editing = None;
        draft.invalid_ms = false;
    }

    fn macro_remove(&mut self, index: usize) {
        let draft = &mut self.macro_draft;
        if index < draft.steps.len() {
            draft.steps.remove(index);
            draft.editing = None;
        }
    }

    fn macro_move_up(&mut self, index: usize) {
        if index > 0 && index < self.macro_draft.steps.len() {
            self.macro_draft.steps.swap(index - 1, index);
            self.macro_draft.editing = None;
        }
    }

    fn macro_move_down(&mut self, index: usize) {
        if index + 1 < self.macro_draft.steps.len() {
            self.macro_draft.steps.swap(index, index + 1);
            self.macro_draft.editing = None;
        }
    }
}

/// The time a step carries: how long it holds, or how long it waits.
fn step_ms(step: MacroStep) -> Option<u16> {
    match step {
        MacroStep::Stroke { hold_ms, .. } => Some(hold_ms),
        MacroStep::Delay(ms) => Some(ms),
        MacroStep::Press(_) | MacroStep::Release(_) => None,
    }
}

fn input_label(input: MacroInput) -> SharedString {
    match input {
        MacroInput::Key(chord) => chord.to_string().into(),
        MacroInput::Mouse(button) => action_label(&OnboardAction::Mouse(button)),
    }
}

fn input_icon(input: MacroInput) -> &'static str {
    match input {
        MacroInput::Key(_) => "action-icons/keyboard.svg",
        MacroInput::Mouse(_) => "action-icons/mouse.svg",
    }
}

/// A step's glyph, its line, and the time beside it.
fn step_text(step: MacroStep) -> (&'static str, SharedString, Option<SharedString>) {
    match step {
        MacroStep::Stroke { input, hold_ms } => (
            input_icon(input),
            input_label(input),
            Some(tr!("device.onboard_macro_held", ms => hold_ms)),
        ),
        MacroStep::Delay(ms) => (
            "action-icons/timer.svg",
            tr!("device.onboard_macro_wait", ms => ms),
            None,
        ),
        MacroStep::Press(input) => (
            input_icon(input),
            tr!("device.onboard_macro_press", input => input_label(input)),
            None,
        ),
        MacroStep::Release(input) => (
            input_icon(input),
            tr!("device.onboard_macro_release", input => input_label(input)),
            None,
        ),
    }
}

/// Run `edit` on the view and repaint.
fn update(
    view: &Entity<OnboardModelView>,
    cx: &mut App,
    edit: impl FnOnce(&mut OnboardModelView, &mut Context<OnboardModelView>),
) {
    view.update(cx, |view, cx| {
        edit(view, cx);
        cx.notify();
    });
}

/// The Macro picker for the selected button.
pub(super) fn macro_editor(
    drawing: &OnboardDrawing,
    target: OnboardTarget,
    current: &OnboardBinding,
    picking: &Picking<'_>,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    let Picking {
        picker,
        draft,
        chosen_key,
    } = *picking;
    let recording = draft.recording.is_some();
    let record_view = view.clone();
    let clear_view = view.clone();
    let record = control_button("onboard-macro-record")
        .debug_selector(|| "onboard-macro-record".into())
        .flex_1()
        .label(if recording {
            tr!("device.onboard_macro_stop")
        } else {
            tr!("device.onboard_macro_record")
        })
        .when(recording, ButtonVariants::danger)
        .on_click(move |_, window, cx| {
            update(&record_view, cx, |view, cx| {
                if view.macro_draft.recording.is_some() {
                    view.macro_stop_recording();
                } else {
                    view.macro_start_recording(window, cx);
                }
            });
        });
    let clear = control_button("onboard-macro-clear")
        .flex_1()
        .label(tr!("device.onboard_macro_clear"))
        .disabled(recording || draft.steps.is_empty())
        .on_click(move |_, _, cx| {
            update(&clear_view, cx, |view, _| {
                view.macro_draft.steps.clear();
                view.macro_draft.editing = None;
            });
        });

    let save_view = view.clone();
    let key = drawing.key.clone();
    let slot = target.slot;
    let unsaved = draft.unsaved(current);
    let save = control_button("onboard-macro-save")
        .debug_selector(|| "onboard-macro-save".into())
        .primary()
        .w_full()
        .label(tr!("device.onboard_macro_save"))
        .disabled(!drawing.reachable || recording || draft.steps.is_empty() || !unsaved)
        .on_click(move |_, _, cx| {
            let steps = save_view.read(cx).macro_draft.steps.clone();
            AppState::apply(cx, |state| {
                state.set_onboard_button(&key, slot, OnboardAction::Macro(OnboardMacro { steps }))
            });
        });

    v_flex()
        .gap_3()
        .child(h_flex().gap_2().child(record).child(clear))
        .when(recording, |editor| {
            editor.child(record_pad(draft, view, pal))
        })
        .child(
            v_flex()
                .gap_1()
                .child(editor_section(tr!("device.onboard_macro_steps"), pal))
                .child(step_list(draft, view, pal)),
        )
        .when(!recording, |editor| {
            editor.child(add_step(draft, picker, chosen_key, view, pal))
        })
        .when(draft.invalid_ms, |editor| {
            editor.child(
                div()
                    .text_caption()
                    .text_color(rgb(0x00ef_4444))
                    .child(tr!("device.onboard_macro_ms_invalid")),
            )
        })
        .when(unsaved && !draft.steps.is_empty(), |editor| {
            editor.child(
                div()
                    .debug_selector(|| "onboard-macro-unsaved".into())
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(tr!("device.onboard_macro_unsaved")),
            )
        })
        .child(save)
}

/// The box that takes the keys and clicks while recording.
fn record_pad(
    draft: &MacroDraft,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    let key_up = view.clone();
    let modifiers = view.clone();
    let mouse_down = view.clone();
    let count = draft
        .recording
        .as_ref()
        .map_or(0, |recording| recording.events.len());
    let mut pad = div()
        .id("onboard-macro-pad")
        .debug_selector(|| "onboard-macro-pad".into())
        .track_focus(&draft.pad)
        .min_h(px(96.))
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .justify_center()
        .rounded(pal.control_radius)
        .border_2()
        .border_dashed()
        .border_color(rgb(ACCENT_BLUE))
        .bg(pal.control)
        .child(
            div()
                .text_body()
                .text_color(pal.text_primary)
                .child(tr!("device.onboard_macro_recording")),
        )
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(format!("{count}")),
        )
        .on_key_up(move |event: &KeyUpEvent, _, cx| {
            cx.stop_propagation();
            let key = recorded_key(&event.keystroke.key);
            debug!(key = %event.keystroke.key, recorded = ?key, "macro recording: key up");
            let Some(key) = key else {
                return;
            };
            update(&key_up, cx, |view, _| {
                view.macro_record(|recording, now| {
                    recording.release(MacroInput::Key(HidKeyChord::key(key)), now);
                });
            });
        })
        .on_modifiers_changed(move |event: &ModifiersChangedEvent, _, cx| {
            let pressed = event.modifiers;
            update(&modifiers, cx, |view, _| {
                view.macro_record(|recording, now| recording.modifiers_changed(pressed, now));
            });
        })
        .on_any_mouse_down(move |event: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            let button = recorded_button(event.button);
            update(&mouse_down, cx, |view, cx| {
                // A click keeps the pad focused, so keys keep landing here.
                window.focus(&view.macro_draft.pad, cx);
                view.macro_record(|recording, now| {
                    recording.press(MacroInput::Mouse(button), now);
                });
            });
        });
    for button in [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Navigate(NavigationDirection::Back),
        MouseButton::Navigate(NavigationDirection::Forward),
    ] {
        let view = view.clone();
        pad = pad.on_mouse_up(button, move |event: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
            let button = recorded_button(event.button);
            update(&view, cx, |view, _| {
                view.macro_record(|recording, now| {
                    recording.release(MacroInput::Mouse(button), now);
                });
            });
        });
    }
    pad
}

/// The steps, each with its time to edit and buttons to move or drop it.
fn step_list(
    draft: &MacroDraft,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    if draft.steps.is_empty() {
        return div()
            .debug_selector(|| "onboard-macro-empty".into())
            .py_2()
            .text_body()
            .text_color(pal.text_muted)
            .child(tr!("device.onboard_macro_empty"))
            .into_any_element();
    }
    let last = draft.steps.len() - 1;
    v_flex()
        .gap_1()
        .children(
            draft
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| step_item(index, *step, last, draft, view, pal)),
        )
        .into_any_element()
}

/// One step's row, and under it the time editor while it is being edited.
fn step_item(
    index: usize,
    step: MacroStep,
    last: usize,
    draft: &MacroDraft,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> gpui::Div {
    let locked = draft.recording.is_some();
    let (icon, title, caption) = step_text(step);
    let editable = step_ms(step).is_some() && !locked;
    let editing = draft.editing == Some(index);
    let icon_button = |id: &'static str, glyph: IconName, enabled: bool| {
        Button::new((id, index))
            .ghost()
            .xsmall()
            .icon(glyph)
            .disabled(!enabled || locked)
    };
    let (edit_view, up_view, down_view, remove_view) =
        (view.clone(), view.clone(), view.clone(), view.clone());
    let row = h_flex()
        .id(("onboard-macro-step", index))
        .debug_selector(move || format!("onboard-macro-step-{index}"))
        .w_full()
        .min_w_0()
        .gap_2()
        .px_2()
        .py_1()
        .items_center()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(if editing {
            rgb(ACCENT_BLUE).into()
        } else {
            pal.border
        })
        .bg(pal.control)
        .when(editable, |row| {
            row.cursor_pointer().on_click(move |_, window, cx| {
                update(&edit_view, cx, |view, cx| {
                    view.macro_edit(index, window, cx);
                });
            })
        })
        .child(
            div()
                .w(px(18.))
                .flex_none()
                .text_caption()
                .text_color(pal.text_muted)
                .child(format!("{}", index + 1)),
        )
        .child(
            svg()
                .path(icon)
                .size_4()
                .flex_none()
                .text_color(pal.text_muted),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().text_body().child(title))
                .children(caption.map(|caption| {
                    div()
                        .text_caption()
                        .text_color(pal.text_muted)
                        .child(caption)
                })),
        )
        .child(
            icon_button("onboard-macro-up", IconName::ChevronUp, index > 0).on_click(
                move |_, _, cx| update(&up_view, cx, |view, _| view.macro_move_up(index)),
            ),
        )
        .child(
            icon_button("onboard-macro-down", IconName::ChevronDown, index < last).on_click(
                move |_, _, cx| {
                    update(&down_view, cx, |view, _| view.macro_move_down(index));
                },
            ),
        )
        .child(
            icon_button("onboard-macro-remove", IconName::Close, true).on_click(move |_, _, cx| {
                update(&remove_view, cx, |view, _| view.macro_remove(index));
            }),
        );
    let apply_view = view.clone();
    v_flex().gap_1().child(row).when(editing, |item| {
        item.child(
            h_flex()
                .gap_2()
                .pl(px(26.))
                .child(div().flex_1().child(control_input(&draft.edit_ms)))
                .child(
                    control_button(("onboard-macro-set", index))
                        .label(tr!("device.onboard_macro_set"))
                        .on_click(move |_, _, cx| {
                            update(&apply_view, cx, OnboardModelView::macro_apply_edit);
                        }),
                ),
        )
    })
}

/// The Add step button, and once pressed the form for one step: a
/// keystroke or mouse button with how long to hold it, or a wait.
fn add_step(
    draft: &MacroDraft,
    picker: &PickerState,
    chosen_key: Option<HidKey>,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    let toggle_view = view.clone();
    let open = draft.adding.is_some();
    let toggle = control_button("onboard-macro-add")
        .debug_selector(|| "onboard-macro-add".into())
        .outline()
        .w_full()
        .icon(if open {
            IconName::Minus
        } else {
            IconName::Plus
        })
        .label(tr!("device.onboard_macro_add"))
        .on_click(move |_, _, cx| {
            update(&toggle_view, cx, |view, _| {
                view.macro_draft.adding = match view.macro_draft.adding {
                    Some(_) => None,
                    None => Some(AddKind::Keystroke),
                };
            });
        });
    let Some(kind) = draft.adding else {
        return v_flex().child(toggle);
    };
    let kind_button = |id: &'static str, label: SharedString, choice: AddKind| {
        let view = view.clone();
        control_button(id)
            .debug_selector(move || id.to_string())
            .flex_1()
            .label(label)
            .when(kind == choice, ButtonVariants::primary)
            .on_click(move |_, _, cx| {
                update(&view, cx, |view, _| view.macro_draft.adding = Some(choice));
            })
    };
    let form = match kind {
        AddKind::Keystroke => {
            keystroke_form(draft, picker, chosen_key, view, pal).into_any_element()
        }
        AddKind::MouseButton => mouse_form(draft, view, pal).into_any_element(),
        AddKind::Delay => v_flex()
            .gap_1()
            .child(editor_section(tr!("device.onboard_macro_delay_ms"), pal))
            .child(control_input(&draft.delay_ms))
            .into_any_element(),
    };
    let add_view = view.clone();
    v_flex().gap_3().child(toggle).child(
        v_flex()
            .gap_3()
            .p_3()
            .rounded(pal.control_radius)
            .border_1()
            .border_color(pal.border)
            .child(
                h_flex()
                    .gap_1()
                    .child(kind_button(
                        "onboard-macro-add-key",
                        tr!("device.onboard_macro_add_key"),
                        AddKind::Keystroke,
                    ))
                    .child(kind_button(
                        "onboard-macro-add-mouse",
                        tr!("device.onboard_macro_add_mouse"),
                        AddKind::MouseButton,
                    ))
                    .child(kind_button(
                        "onboard-macro-add-delay",
                        tr!("device.onboard_macro_add_delay"),
                        AddKind::Delay,
                    )),
            )
            .child(form)
            .child(
                control_button("onboard-macro-add-confirm")
                    .debug_selector(|| "onboard-macro-add-confirm".into())
                    .w_full()
                    .icon(IconName::Plus)
                    .label(tr!("device.onboard_macro_add_confirm"))
                    .on_click(move |_, _, cx| {
                        update(&add_view, cx, OnboardModelView::macro_add);
                    }),
            ),
    )
}

/// A keystroke: modifiers, the key, and how long to hold it.
fn keystroke_form(
    draft: &MacroDraft,
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
            control_button(SharedString::from(format!("onboard-macro-mod-{name}")))
                .label(name)
                .when(picker.modifiers.contains(flag), ButtonVariants::primary)
                .on_click(move |_, _, cx| {
                    update(&view, cx, |view, _| view.toggle_modifier(flag));
                })
        }));
    let caption = match picked_chord(picker.modifiers, chosen_key) {
        Some(chord) => SharedString::from(chord.to_string()),
        None if picker.key_missing => tr!("device.onboard_key_choose"),
        None => SharedString::default(),
    };
    v_flex()
        .gap_2()
        .child(editor_section(tr!("device.onboard_key_hold"), pal))
        .child(toggles)
        .child(editor_section(tr!("device.onboard_key_press"), pal))
        .child(control_select(&picker.keys).placeholder(tr!("device.onboard_key_search")))
        .when(!caption.is_empty(), |form| {
            form.child(
                div()
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(caption),
            )
        })
        .child(editor_section(tr!("device.onboard_macro_hold_ms"), pal))
        .child(control_input(&draft.hold_ms))
}

/// A mouse button and how long to hold it.
fn mouse_form(
    draft: &MacroDraft,
    view: &Entity<OnboardModelView>,
    pal: Palette,
) -> impl IntoElement {
    let buttons = [
        OnboardMouseButton::Left,
        OnboardMouseButton::Right,
        OnboardMouseButton::Middle,
        OnboardMouseButton::Back,
        OnboardMouseButton::Forward,
    ];
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(buttons.into_iter().enumerate().map(|(index, button)| {
                    let view = view.clone();
                    control_button(("onboard-macro-mouse", index))
                        .label(action_label(&OnboardAction::Mouse(button)))
                        .when(draft.add_mouse == button, ButtonVariants::primary)
                        .on_click(move |_, _, cx| {
                            update(&view, cx, |view, _| view.macro_draft.add_mouse = button);
                        })
                })),
        )
        .child(editor_section(tr!("device.onboard_macro_hold_ms"), pal))
        .child(control_input(&draft.hold_ms))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn key(name: &str) -> MacroInput {
        MacroInput::Key(HidKeyChord::key(HidKey::from_name(name).expect("a key")))
    }

    #[test]
    fn a_recording_times_each_event_from_the_last() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut recording = Recording::default();
        recording.press(key("A"), at(0));
        recording.release(key("A"), at(80));
        recording.press(MacroInput::Mouse(OnboardMouseButton::Left), at(200));
        recording.release(MacroInput::Mouse(OnboardMouseButton::Left), at(230));
        assert_eq!(
            recording.finish(at(500)),
            [
                MacroStep::Stroke {
                    input: key("A"),
                    hold_ms: 80
                },
                MacroStep::Delay(120),
                MacroStep::Stroke {
                    input: MacroInput::Mouse(OnboardMouseButton::Left),
                    hold_ms: 30
                },
            ]
        );
    }

    #[test]
    fn modifiers_record_as_their_own_keys_and_never_stay_held() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut recording = Recording::default();
        // A key held before recording began is never released by it.
        recording.release(key("B"), at(0));
        recording.modifiers_changed(
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
            at(10),
        );
        recording.press(key("C"), at(40));
        recording.release(key("C"), at(60));
        // Recording stops with Ctrl still down.
        assert_eq!(
            recording.finish(at(100)),
            [
                MacroStep::Press(key("Left Ctrl")),
                MacroStep::Delay(30),
                MacroStep::Stroke {
                    input: key("C"),
                    hold_ms: 20
                },
                MacroStep::Delay(40),
                MacroStep::Release(key("Left Ctrl")),
            ]
        );
    }

    #[test]
    fn gpui_key_names_map_to_standard_keys() {
        assert_eq!(recorded_key("a"), HidKey::from_name("A"));
        assert_eq!(recorded_key("pageup"), HidKey::from_name("Page Up"));
        assert_eq!(recorded_key("enter"), HidKey::from_name("Enter"));
        assert_eq!(recorded_key("f13"), HidKey::from_name("F13"));
        assert_eq!(recorded_key("menu"), HidKey::from_name("Application"));
        assert_eq!(recorded_key("back"), None);
    }
}
