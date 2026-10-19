//! The app-facing view of `0x8100` button bindings: the active profile's
//! buttons as [`OnboardButtons`], and one button rebound to an
//! [`OnboardAction`], answered with the read-back.

use std::sync::Arc;

use hidpp::channel::HidppChannel;
use hidpp::device::Device;
use hidpp::feature::onboard_profiles::OnboardProfilesFeature;
use hidpp::feature::onboard_profiles::layout::{
    BINDING_SLOTS, BindingLayer, ButtonBinding, ButtonFunction, KeyModifiers, MouseButtons,
    ProfileSector, ROM_SECTOR_BASE, UnknownBinding,
};
use hidpp::protocol::v20::Hidpp20Error;
use openlogi_core::binding::{CommandModifier, KeyCombo};
use openlogi_core::hid::{
    OnboardAction, OnboardBinding, OnboardButton, OnboardButtons, OnboardFunction,
    OnboardMouseButton,
};
use tracing::debug;

use super::{FEATURE, OnboardBindingChange, OnboardWriteError, remap_on};
use crate::SharedChannel;
use crate::backend::HidBackend;
use crate::channel::route::DeviceRoute;
use crate::write::{HidppOperation, WriteError, classify_hidpp_error, open_feature, with_route};

/// Read the active onboard profile's buttons of the mouse on `route`.
pub async fn get_onboard_buttons(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<OnboardButtons, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let feature = open(&channel, index).await?;
        read_buttons(&feature, HidppOperation::ReadOnboardButtons).await
    })
    .await
}

/// [`get_onboard_buttons`] on an already-open [`SharedChannel`].
pub async fn get_onboard_buttons_on(shared: &SharedChannel) -> Result<OnboardButtons, WriteError> {
    let feature = open(shared.channel(), shared.device_index()).await?;
    read_buttons(&feature, HidppOperation::ReadOnboardButtons).await
}

/// Bind `button` (zero-based) of the active onboard profile of the mouse on
/// `route` to `action`, and answer with the buttons the mouse reads back.
///
/// `command` says what a key chord's Command modifier stands for on this
/// computer; the mouse stores the physical key. Refused with
/// [`WriteError::OnboardProfileNotWritable`] or
/// [`WriteError::OnboardButtonUnsupported`] before anything is written; a
/// write the mouse does not keep fails with
/// [`WriteError::OnboardBindingNotApplied`]. Nothing else in the profile
/// changes.
pub async fn set_onboard_button(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
    button: u8,
    action: OnboardAction,
    command: CommandModifier,
) -> Result<OnboardButtons, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let feature = open(&channel, index).await?;
        set_button(&feature, button, &action, command).await
    })
    .await
}

/// [`set_onboard_button`] on an already-open [`SharedChannel`].
pub async fn set_onboard_button_on(
    shared: &SharedChannel,
    button: u8,
    action: &OnboardAction,
    command: CommandModifier,
) -> Result<OnboardButtons, WriteError> {
    let feature = open(shared.channel(), shared.device_index()).await?;
    set_button(&feature, button, action, command).await
}

async fn open(
    channel: &Arc<HidppChannel>,
    index: u8,
) -> Result<Arc<OnboardProfilesFeature>, WriteError> {
    let mut device = Device::new(Arc::clone(channel), index)
        .await
        .map_err(|_| WriteError::DeviceUnreachable { index })?;
    open_feature::<OnboardProfilesFeature>(&mut device).await
}

pub(super) async fn set_button(
    feature: &OnboardProfilesFeature,
    button: u8,
    action: &OnboardAction,
    command: CommandModifier,
) -> Result<OnboardButtons, WriteError> {
    let change = OnboardBindingChange {
        layer: BindingLayer::Primary,
        slot: usize::from(button),
        binding: binding_to_device(action, command),
    };
    let written = remap_on(feature, &[change])
        .await
        .map_err(|e| write_error(e, button))?;
    debug!(
        sector = written.sector,
        button,
        changed = written.changed(),
        "onboard button written"
    );
    read_buttons(feature, HidppOperation::WriteOnboardButton).await
}

/// The wire error a refused or failed remap of `button` reports.
fn write_error(error: OnboardWriteError, button: u8) -> WriteError {
    match error {
        OnboardWriteError::Write(error) => error,
        OnboardWriteError::NoSuchButton { .. } => WriteError::OnboardButtonUnsupported { button },
        OnboardWriteError::VerifyFailed(_) => WriteError::OnboardBindingNotApplied { button },
        OnboardWriteError::UnsupportedLayout { .. }
        | OnboardWriteError::DirectoryInvalid
        | OnboardWriteError::ActiveProfileNotWritable(_)
        | OnboardWriteError::ActiveProfileCorrupt(_)
        | OnboardWriteError::Edit(_)
        | OnboardWriteError::InvalidBackup { .. } => WriteError::OnboardProfileNotWritable,
    }
}

pub(super) async fn read_buttons(
    feature: &OnboardProfilesFeature,
    operation: HidppOperation,
) -> Result<OnboardButtons, WriteError> {
    let fail = |e: Hidpp20Error| classify_hidpp_error(e, operation, FEATURE);
    let info = feature.get_info().await.map_err(fail)?;
    if !info.layout_supported() {
        return Err(WriteError::OnboardProfileNotWritable);
    }
    let sector = feature.get_current_profile_sector().await.map_err(fail)?;
    let active = feature
        .read_sector(sector, info.sector_size)
        .await
        .map_err(fail)?;
    let active =
        ProfileSector::decode(&active).map_err(|_| WriteError::OnboardProfileNotWritable)?;
    // The factory profile names each physical button. A mouse without one,
    // or one that cannot be read, still shows its current bindings.
    let factory = if info.rom_profile_count > 0 {
        feature
            .read_sector(ROM_SECTOR_BASE + 1, info.sector_size)
            .await
            .ok()
            .and_then(|raw| ProfileSector::decode(&raw).ok())
    } else {
        None
    };
    let count = usize::from(info.button_count).min(BINDING_SLOTS);
    let buttons = (0..count)
        .map(|slot| OnboardButton {
            binding: binding_from_device(active.buttons[slot]),
            factory: factory
                .as_ref()
                .and_then(|profile| match profile.buttons[slot] {
                    Ok(ButtonBinding::Unassigned) => None,
                    other => Some(binding_from_device(other)),
                }),
        })
        .collect();
    Ok(OnboardButtons { sector, buttons })
}

/// The app's view of a binding read from the mouse. An erased slot does
/// nothing, so it reads as [`OnboardAction::Disabled`].
pub(crate) fn binding_from_device(
    binding: Result<ButtonBinding, UnknownBinding>,
) -> OnboardBinding {
    let action = |action| OnboardBinding::Action(action);
    let raw = |binding: ButtonBinding| {
        binding.encode().map_or(
            OnboardBinding::Unrecognized([0xff; 4]),
            OnboardBinding::Unrecognized,
        )
    };
    match binding {
        Err(UnknownBinding(bytes)) => OnboardBinding::Unrecognized(bytes),
        Ok(ButtonBinding::Unassigned | ButtonBinding::NoAction) => action(OnboardAction::Disabled),
        Ok(ButtonBinding::Macro { .. }) => OnboardBinding::Macro,
        Ok(ButtonBinding::Consumer(usage)) => action(OnboardAction::Consumer(usage)),
        Ok(binding @ ButtonBinding::Mouse(buttons)) => mouse_button(buttons).map_or_else(
            || raw(binding),
            |button| action(OnboardAction::Mouse(button)),
        ),
        Ok(binding @ ButtonBinding::Key { modifiers, usage }) => {
            KeyCombo::from_hid(modifiers.bits(), usage)
                .map_or_else(|| raw(binding), |combo| action(OnboardAction::Key(combo)))
        }
        Ok(binding @ ButtonBinding::Function { function, .. }) => match function {
            ButtonFunction::NoAction => action(OnboardAction::Disabled),
            other => onboard_function(other).map_or_else(
                || raw(binding),
                |function| action(OnboardAction::Function(function)),
            ),
        },
    }
}

/// The bytes the mouse stores for `action`.
pub(crate) fn binding_to_device(action: &OnboardAction, command: CommandModifier) -> ButtonBinding {
    match action {
        OnboardAction::Disabled => ButtonBinding::NoAction,
        OnboardAction::Mouse(button) => ButtonBinding::Mouse(match button {
            OnboardMouseButton::Left => MouseButtons::LEFT,
            OnboardMouseButton::Right => MouseButtons::RIGHT,
            OnboardMouseButton::Middle => MouseButtons::MIDDLE,
            OnboardMouseButton::Back => MouseButtons::BACK,
            OnboardMouseButton::Forward => MouseButtons::FORWARD,
        }),
        OnboardAction::Key(combo) => ButtonBinding::Key {
            modifiers: KeyModifiers::from_bits_retain(combo.hid_modifiers(command)),
            usage: combo.key().code(),
        },
        OnboardAction::Consumer(usage) => ButtonBinding::Consumer(*usage),
        OnboardAction::Function(function) => ButtonBinding::Function {
            function: device_function(*function),
            data: 0,
        },
    }
}

/// A single standard mouse button, or `None` for a chord or an extra button.
fn mouse_button(buttons: MouseButtons) -> Option<OnboardMouseButton> {
    Some(match buttons {
        MouseButtons::LEFT => OnboardMouseButton::Left,
        MouseButtons::RIGHT => OnboardMouseButton::Right,
        MouseButtons::MIDDLE => OnboardMouseButton::Middle,
        MouseButtons::BACK => OnboardMouseButton::Back,
        MouseButtons::FORWARD => OnboardMouseButton::Forward,
        _ => return None,
    })
}

/// The app's name for a firmware function, or `None` for one the app does
/// not offer (profile select, mode switch, host button).
fn onboard_function(function: ButtonFunction) -> Option<OnboardFunction> {
    Some(match function {
        ButtonFunction::TiltLeft => OnboardFunction::TiltLeft,
        ButtonFunction::TiltRight => OnboardFunction::TiltRight,
        ButtonFunction::NextDpi => OnboardFunction::NextDpi,
        ButtonFunction::PreviousDpi => OnboardFunction::PreviousDpi,
        ButtonFunction::CycleDpi => OnboardFunction::CycleDpi,
        ButtonFunction::DefaultDpi => OnboardFunction::DefaultDpi,
        ButtonFunction::ShiftDpi => OnboardFunction::ShiftDpi,
        ButtonFunction::NextProfile => OnboardFunction::NextProfile,
        ButtonFunction::PreviousProfile => OnboardFunction::PreviousProfile,
        ButtonFunction::CycleProfile => OnboardFunction::CycleProfile,
        ButtonFunction::GShift => OnboardFunction::GShift,
        ButtonFunction::BatteryStatus => OnboardFunction::BatteryStatus,
        ButtonFunction::ScrollDown => OnboardFunction::ScrollDown,
        ButtonFunction::ScrollUp => OnboardFunction::ScrollUp,
        ButtonFunction::NoAction
        | ButtonFunction::ProfileSelect
        | ButtonFunction::ModeSwitch
        | ButtonFunction::HostButton => return None,
    })
}

fn device_function(function: OnboardFunction) -> ButtonFunction {
    match function {
        OnboardFunction::TiltLeft => ButtonFunction::TiltLeft,
        OnboardFunction::TiltRight => ButtonFunction::TiltRight,
        OnboardFunction::NextDpi => ButtonFunction::NextDpi,
        OnboardFunction::PreviousDpi => ButtonFunction::PreviousDpi,
        OnboardFunction::CycleDpi => ButtonFunction::CycleDpi,
        OnboardFunction::DefaultDpi => ButtonFunction::DefaultDpi,
        OnboardFunction::ShiftDpi => ButtonFunction::ShiftDpi,
        OnboardFunction::NextProfile => ButtonFunction::NextProfile,
        OnboardFunction::PreviousProfile => ButtonFunction::PreviousProfile,
        OnboardFunction::CycleProfile => ButtonFunction::CycleProfile,
        OnboardFunction::GShift => ButtonFunction::GShift,
        OnboardFunction::BatteryStatus => ButtonFunction::BatteryStatus,
        OnboardFunction::ScrollDown => ButtonFunction::ScrollDown,
        OnboardFunction::ScrollUp => ButtonFunction::ScrollUp,
    }
}
