//! Onboard buttons: a pick writes the mouse through the agent — only one its
//! last reading takes — and nothing is persisted on this computer.

use super::fn_lock::drain;
use super::*;
use crate::services::ipc::{Command, ReadOnboardButtons, SetOnboardButton};
use crate::state::events::StateEvents;
use crate::state::onboard_buttons::needs_write;
use crate::state::{OnboardButtonRefusal, OnboardButtonsLoad};
use openlogi_core::hid::{
    DeviceRoute, OnboardAction, OnboardBinding, OnboardButton, OnboardButtons, OnboardFunction,
    OnboardMouseButton,
};

fn action(action: OnboardAction) -> OnboardBinding {
    OnboardBinding::Action(action)
}

/// Three buttons of a G502 X profile: back, a G HUB macro on DPI shift, and
/// DPI up.
fn buttons() -> OnboardButtons {
    OnboardButtons {
        sector: 1,
        buttons: vec![
            OnboardButton {
                binding: action(OnboardAction::Mouse(OnboardMouseButton::Back)),
                factory: Some(action(OnboardAction::Mouse(OnboardMouseButton::Back))),
            },
            OnboardButton {
                binding: OnboardBinding::Macro,
                factory: Some(action(OnboardAction::Function(OnboardFunction::ShiftDpi))),
            },
            OnboardButton {
                binding: action(OnboardAction::Function(OnboardFunction::NextDpi)),
                factory: Some(action(OnboardAction::Function(OnboardFunction::NextDpi))),
            },
        ],
    }
}

/// `buttons` with `button` bound to `to`.
fn bound(mut buttons: OnboardButtons, button: u8, to: OnboardAction) -> OnboardButtons {
    buttons.buttons[usize::from(button)].binding = action(to);
    buttons
}

const VOLUME_UP: OnboardAction = OnboardAction::Consumer(0x00e9);
const CYCLE: OnboardAction = OnboardAction::Function(OnboardFunction::CycleProfile);

fn mouse_inventory(onboard: bool) -> DeviceInventory {
    let mut inventory = direct_inventory([0xa3, 0x93, 0xca, 0xe0]);
    inventory.paired[0]
        .capabilities
        .as_mut()
        .expect("the fixture device has capabilities")
        .onboard_buttons = onboard;
    inventory
}

fn mouse_state(onboard: bool) -> (AppState, tokio::sync::mpsc::UnboundedReceiver<Command>) {
    let resolver = AssetResolver::new();
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    let state = AppState::new(Sources {
        inventories: &[mouse_inventory(onboard)],
        ..Sources::in_memory(Config::ephemeral(), &resolver, commands)
    });
    (state, receiver)
}

/// Answer the onboard-button read the state has queued, returning the route
/// it was asked on. Every other read is left unanswered.
fn answer_read(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    reading: &OnboardButtons,
) -> Option<DeviceRoute> {
    let mut answered = None;
    for command in drain(receiver) {
        if let Command::ReadOnboardButtons(ReadOnboardButtons { route, reply }) = command {
            assert!(answered.is_none(), "one onboard-button read at a time");
            let _ = reply.send(Ok(reading.clone()));
            answered = Some(route);
        }
    }
    answered
}

/// The onboard-button writes among the queued commands — button, action and
/// the number they were sent under — oldest first. Panics on a config
/// reload: a pick never touches `config.toml`.
fn sent(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
) -> Vec<(u8, OnboardAction, u64)> {
    drain(receiver)
        .into_iter()
        .filter_map(|command| match command {
            Command::SetOnboardButton(SetOnboardButton {
                button,
                action,
                write,
                ..
            }) => Some((button, action, write)),
            Command::ReloadConfig(_) => panic!("a pick does not reload the config"),
            _ => None,
        })
        .collect()
}

fn install(state: AppState, cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let runtime: Arc<dyn swr_core::Runtime> = Arc::new(swr_gpui::GpuiRuntime::new(cx));
        let client = swr_core::SwrClient::builder().build(runtime.clone());
        let state = cx.new(|_| {
            let mut state = state;
            state.connect_device_reads(client, runtime);
            state
        });
        AppState::set_global(state, cx);
        AppState::load_current_device_reads(cx);
    });
    cx.run_until_parked();
}

fn installed_and_read(
    cx: &mut gpui::TestAppContext,
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    state: AppState,
    reading: OnboardButtons,
) -> (DeviceKey, DeviceRoute) {
    let key = state.current_record().expect("mouse").device_key();
    install(state, cx);
    let route = answer_read(receiver, &reading).expect("the state read the mouse's buttons");
    cx.run_until_parked();
    assert_eq!(current_load(cx), Load::Ready(Arc::new(reading)));
    (key, route)
}

fn pick(
    cx: &mut gpui::TestAppContext,
    key: &DeviceKey,
    button: u8,
    to: OnboardAction,
) -> StateEvents {
    cx.update(|cx| AppState::update(cx, |state, _| state.set_onboard_button(key, button, to)))
}

fn written(
    cx: &mut gpui::TestAppContext,
    key: &DeviceKey,
    route: &DeviceRoute,
    write: u64,
    result: Result<OnboardButtons, WriteError>,
) -> StateEvents {
    cx.update(|cx| {
        AppState::update(cx, |state, _| {
            state.apply_onboard_button_written(key, route, write, result)
        })
    })
}

fn current_load(cx: &gpui::TestAppContext) -> OnboardButtonsLoad {
    cx.read(|cx| {
        AppState::try_read(cx)
            .expect("state installed")
            .current_onboard_buttons_load()
    })
}

fn current_refusal(cx: &gpui::TestAppContext) -> Option<OnboardButtonRefusal> {
    cx.read(|cx| {
        AppState::try_read(cx)
            .expect("state installed")
            .current_onboard_button_refusal()
    })
}

#[test]
fn a_write_is_needed_only_for_a_change_to_a_button_the_mouse_has() {
    let buttons = buttons();
    assert!(needs_write(&buttons, 0, &VOLUME_UP));
    assert!(!needs_write(
        &buttons,
        0,
        &OnboardAction::Mouse(OnboardMouseButton::Back)
    ));
    assert!(needs_write(&buttons, 1, &CYCLE), "a macro can be replaced");
    assert!(!needs_write(&buttons, 3, &VOLUME_UP), "no such button");
}

#[gpui::test]
fn a_mouse_without_onboard_buttons_is_never_asked(cx: &mut gpui::TestAppContext) {
    let (state, mut receiver) = mouse_state(false);
    let key = state.current_record().expect("mouse").device_key();
    install(state, cx);
    assert_eq!(answer_read(&mut receiver, &buttons()), None);
    let _ = pick(cx, &key, 0, VOLUME_UP);
    assert!(sent(&mut receiver).is_empty());
}

#[test]
fn a_pick_before_the_mouse_was_read_writes_nothing() {
    let (mut state, mut receiver) = mouse_state(true);
    let key = state.current_record().expect("mouse").device_key();
    let _ = state.set_onboard_button(&key, 0, VOLUME_UP);
    assert!(sent(&mut receiver).is_empty());
}

#[gpui::test]
fn a_pick_shows_at_once_and_writes_the_mouse_once(cx: &mut gpui::TestAppContext) {
    let (state, mut receiver) = mouse_state(true);
    let (key, route) = installed_and_read(cx, &mut receiver, state, buttons());

    assert_eq!(
        pick(cx, &key, 1, CYCLE),
        [StateEvent::OnboardButtonsChanged(key.clone())]
    );
    // The fixture mouse's other reads are retried on their own; only writes
    // and config reloads matter here.
    let commands: Vec<Command> = drain(&mut receiver)
        .into_iter()
        .filter(|command| {
            matches!(
                command,
                Command::SetOnboardButton(_) | Command::ReloadConfig(_)
            )
        })
        .collect();
    let [Command::SetOnboardButton(write)] = &commands[..] else {
        panic!(
            "one write and no config reload, got {} commands",
            commands.len()
        );
    };
    assert_eq!(
        (&write.route, write.button, &write.action, &write.key),
        (&route, 1, &CYCLE, &key)
    );
    assert_eq!(
        current_load(cx),
        Load::Ready(Arc::new(bound(buttons(), 1, CYCLE)))
    );

    for (button, to) in [
        (0, OnboardAction::Mouse(OnboardMouseButton::Back)),
        (3, VOLUME_UP),
    ] {
        let _ = pick(cx, &key, button, to);
    }
    assert!(
        sent(&mut receiver).is_empty(),
        "an unchanged button and a missing one write nothing"
    );
}

#[gpui::test]
fn the_read_back_replaces_the_pick_and_a_refusal_is_kept_and_re_read(
    cx: &mut gpui::TestAppContext,
) {
    let (state, mut receiver) = mouse_state(true);
    let (key, route) = installed_and_read(cx, &mut receiver, state, buttons());

    let _ = pick(cx, &key, 0, VOLUME_UP);
    let [(0, _, first)] = sent(&mut receiver)[..] else {
        panic!("one write");
    };
    // The mouse kept something else than the pick: its read-back wins.
    let kept = bound(buttons(), 0, OnboardAction::Disabled);
    let _ = written(cx, &key, &route, first, Ok(kept.clone()));
    assert_eq!(current_load(cx), Load::Ready(Arc::new(kept.clone())));
    assert_eq!(current_refusal(cx), None);

    let _ = pick(cx, &key, 2, CYCLE);
    let [(2, _, second)] = sent(&mut receiver)[..] else {
        panic!("one write");
    };
    let _ = written(
        cx,
        &key,
        &route,
        second,
        Err(WriteError::OnboardBindingNotApplied { button: 2 }),
    );
    assert_eq!(
        current_refusal(cx),
        Some(OnboardButtonRefusal {
            button: 2,
            error: WriteError::OnboardBindingNotApplied { button: 2 },
        })
    );
    assert_eq!(
        current_load(cx),
        Load::Ready(Arc::new(kept)),
        "the refused pick no longer shows"
    );
    cx.run_until_parked();
    assert!(
        answer_read(&mut receiver, &buttons()).is_some(),
        "a refusal asks the mouse again"
    );
}

#[gpui::test]
fn a_stale_or_misrouted_answer_is_dropped(cx: &mut gpui::TestAppContext) {
    let (state, mut receiver) = mouse_state(true);
    let (key, route) = installed_and_read(cx, &mut receiver, state, buttons());

    let _ = pick(cx, &key, 0, VOLUME_UP);
    let _ = pick(cx, &key, 2, CYCLE);
    let writes = sent(&mut receiver);
    let [(0, _, first), (2, _, second)] = writes[..] else {
        panic!("two writes, oldest first: {writes:?}");
    };
    let both = bound(bound(buttons(), 0, VOLUME_UP), 2, CYCLE);

    // The newer answer covers both; the older one, arriving late, is dropped.
    assert_eq!(
        written(cx, &key, &route, second, Ok(both.clone())),
        [StateEvent::OnboardButtonsChanged(key.clone())]
    );
    assert_eq!(
        written(cx, &key, &route, first, Ok(buttons())),
        StateEvents::none()
    );
    assert_eq!(current_load(cx), Load::Ready(Arc::new(both.clone())));

    let elsewhere = DeviceRoute::Unifying {
        receiver_uid: "LIGHTSPEED".into(),
        slot: 1,
    };
    let _ = pick(cx, &key, 1, VOLUME_UP);
    let [(1, _, third)] = sent(&mut receiver)[..] else {
        panic!("one write");
    };
    assert_eq!(
        written(cx, &key, &elsewhere, third, Ok(buttons())),
        StateEvents::none(),
        "an answer read over another connection"
    );
}
