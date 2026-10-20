//! Button remapping on mice that bind their buttons in `0x8100`
//! OnboardProfiles flash (the G-series, e.g. the G502 X LIGHTSPEED) instead
//! of `0x1b04` ReprogControls.
//!
//! Every write here edits the **active user profile** in place: it reads the
//! whole sector, changes only the requested binding slots, reseals the CRC,
//! writes the sector back and reads it again to prove the flash holds exactly
//! what was sent. Everything the edit did not touch — DPI slots, lighting,
//! G HUB macros and their pointers — goes back byte for byte. The caller
//! gets the original bytes so it can keep a backup and restore it.

use std::sync::Arc;

use hidpp::{
    device::Device,
    feature::CreatableFeature,
    feature::onboard_profiles::{
        OnboardProfilesFeature, OnboardProfilesInfo,
        layout::{
            self, BindingEditError, BindingLayer, ButtonBinding, LayoutError, USER_DIRECTORY_SECTOR,
        },
    },
    protocol::v20::Hidpp20Error,
};

use crate::backend::HidBackend;
use crate::channel::route::DeviceRoute;
use crate::write::{HidppOperation, WriteError, classify_hidpp_error, open_feature, with_route};

/// The `0x8100` feature id, for error reports.
const FEATURE: u16 = <OnboardProfilesFeature as CreatableFeature>::ID;

/// One binding to change in the active profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnboardBindingChange {
    /// Which table the slot is in.
    pub layer: BindingLayer,
    /// Zero-based slot; must be below the mouse's reported button count.
    pub slot: usize,
    /// The new binding.
    pub binding: ButtonBinding,
}

/// What a remap or restore did to the active profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnboardSectorWrite {
    /// The sector that was (or would have been) written.
    pub sector: u16,
    /// The sector as it was before; keep it to undo the change.
    pub before: Vec<u8>,
    /// The sector as it is now. Equal to `before` when nothing changed.
    pub after: Vec<u8>,
}

impl OnboardSectorWrite {
    /// Whether the flash was actually rewritten.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// Why an onboard-profile write was refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum OnboardWriteError {
    /// Talking to the device failed.
    #[error(transparent)]
    Write(#[from] WriteError),
    /// The profile store uses a layout this build does not decode, so
    /// editing it could corrupt it.
    #[error(
        "unsupported onboard profile layout (memory model 0x{memory_model:02x}, profile format 0x{profile_format:02x}, macro format 0x{macro_format:02x})"
    )]
    UnsupportedLayout {
        /// Reported memory model.
        memory_model: u8,
        /// Reported profile format.
        profile_format: u8,
        /// Reported macro format.
        macro_format: u8,
    },
    /// The user profile directory failed its CRC or could not be decoded:
    /// the mouse is running from a factory profile, which cannot be written.
    #[error("the user profile directory is invalid; the mouse is running a factory profile")]
    DirectoryInvalid,
    /// The active profile is not one of the user sectors the directory
    /// lists, so it cannot be edited in place.
    #[error("the active profile (sector 0x{0:04x}) is not a writable user profile")]
    ActiveProfileNotWritable(u16),
    /// The active profile sector failed its CRC before the edit; writing a
    /// fresh CRC over it would hide the corruption.
    #[error("the active profile (sector 0x{0:04x}) failed its CRC check; not editing it")]
    ActiveProfileCorrupt(u16),
    /// A slot is past the buttons the mouse reports.
    #[error("slot {slot} is past the mouse's {button_count} buttons")]
    NoSuchButton {
        /// Requested zero-based slot.
        slot: usize,
        /// Buttons the mouse reports.
        button_count: u8,
    },
    /// The edit could not be applied to the sector bytes.
    #[error(transparent)]
    Edit(#[from] BindingEditError),
    /// A sector to restore is not a complete, sealed sector of this mouse.
    #[error("backup is not a valid {expected}-byte sector with a matching CRC")]
    InvalidBackup {
        /// The mouse's sector size.
        expected: u16,
    },
    /// Reading the sector back did not return what was written.
    #[error("sector 0x{0:04x} read back differently from what was written")]
    VerifyFailed(u16),
    /// A macro has more commands than a sector holds.
    #[error("the macro needs {needed} bytes but a sector holds {capacity}")]
    MacroTooLong {
        /// Bytes the macro needs.
        needed: u16,
        /// Bytes a sector has for it.
        capacity: u16,
    },
    /// Every sector past the profiles is bound or holds data that is not a
    /// macro.
    #[error("no sector is free for another macro")]
    MacroNoRoom,
}

/// The checks every write shares, done once the feature is open: a layout
/// this build understands, a valid directory, and an active sector that
/// the directory lists. Returns the store info and the active sector.
async fn writable_active_sector(
    feature: &OnboardProfilesFeature,
) -> Result<(OnboardProfilesInfo, u16), OnboardWriteError> {
    let fail =
        |e: Hidpp20Error| classify_hidpp_error(e, HidppOperation::WriteOnboardButton, FEATURE);
    let info = feature.get_info().await.map_err(fail)?;
    if !info.layout_supported() {
        return Err(OnboardWriteError::UnsupportedLayout {
            memory_model: info.memory_model,
            profile_format: info.profile_format,
            macro_format: info.macro_format,
        });
    }
    let directory = feature
        .read_sector(USER_DIRECTORY_SECTOR, info.sector_size)
        .await
        .map_err(fail)?;
    if !layout::sector_crc_valid(&directory) {
        return Err(OnboardWriteError::DirectoryInvalid);
    }
    let entries = layout::decode_directory(&directory)
        .map_err(|_: LayoutError| OnboardWriteError::DirectoryInvalid)?;
    let active = feature.get_current_profile_sector().await.map_err(fail)?;
    // A user sector is one the directory lists and that sits inside the
    // writable sector range; ROM sectors (0x0100+) never qualify.
    let writable = active != USER_DIRECTORY_SECTOR
        && active < u16::from(info.sector_count)
        && entries.iter().any(|entry| entry.sector == active);
    if !writable {
        return Err(OnboardWriteError::ActiveProfileNotWritable(active));
    }
    Ok((info, active))
}

/// Writes `data` to `sector` and reads it back. Nothing is written when the
/// sector already holds `data`.
async fn write_verified(
    feature: &OnboardProfilesFeature,
    sector: u16,
    before: Vec<u8>,
    data: Vec<u8>,
    sector_size: u16,
) -> Result<OnboardSectorWrite, OnboardWriteError> {
    if data == before {
        return Ok(OnboardSectorWrite {
            sector,
            after: before.clone(),
            before,
        });
    }
    let fail =
        |e: Hidpp20Error| classify_hidpp_error(e, HidppOperation::WriteOnboardButton, FEATURE);
    feature.write_sector(sector, &data).await.map_err(fail)?;
    let after = feature
        .read_sector(sector, sector_size)
        .await
        .map_err(fail)?;
    if after != data {
        return Err(OnboardWriteError::VerifyFailed(sector));
    }
    Ok(OnboardSectorWrite {
        sector,
        before,
        after,
    })
}

/// Changes bindings in the active onboard profile of the mouse on `route`.
///
/// Refuses before writing anything unless the store's layout is supported,
/// its directory is valid, the active profile is a user sector with a valid
/// CRC, and every slot is below the mouse's button count. On success the
/// returned `before` bytes are the backup to restore.
pub async fn remap_onboard_buttons(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
    changes: Vec<OnboardBindingChange>,
) -> Result<OnboardSectorWrite, OnboardWriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let feature = open_feature::<OnboardProfilesFeature>(&mut device).await?;
        Ok(remap_on(&feature, &changes).await)
    })
    .await?
}

async fn remap_on(
    feature: &OnboardProfilesFeature,
    changes: &[OnboardBindingChange],
) -> Result<OnboardSectorWrite, OnboardWriteError> {
    let (info, sector) = writable_active_sector(feature).await?;
    if let Some(change) = changes
        .iter()
        .find(|change| change.slot >= usize::from(info.button_count))
    {
        return Err(OnboardWriteError::NoSuchButton {
            slot: change.slot,
            button_count: info.button_count,
        });
    }
    let fail =
        |e: Hidpp20Error| classify_hidpp_error(e, HidppOperation::WriteOnboardButton, FEATURE);
    let before = feature
        .read_sector(sector, info.sector_size)
        .await
        .map_err(fail)?;
    if !layout::sector_crc_valid(&before) {
        return Err(OnboardWriteError::ActiveProfileCorrupt(sector));
    }
    let mut data = before.clone();
    for change in changes {
        layout::set_binding(&mut data, change.layer, change.slot, change.binding)?;
    }
    layout::seal(&mut data);
    write_verified(feature, sector, before, data, info.sector_size).await
}

/// Writes a backup taken by [`remap_onboard_buttons`] back over the active
/// onboard profile of the mouse on `route`.
///
/// The backup must be exactly one sector long with a valid CRC, and the
/// active profile must pass the same checks a remap does. It is written to
/// the *active* sector, so restore with the same profile selected as when
/// the backup was taken.
pub async fn restore_onboard_profile(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
    backup: Vec<u8>,
) -> Result<OnboardSectorWrite, OnboardWriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let feature = open_feature::<OnboardProfilesFeature>(&mut device).await?;
        Ok(restore_on(&feature, backup).await)
    })
    .await?
}

async fn restore_on(
    feature: &OnboardProfilesFeature,
    backup: Vec<u8>,
) -> Result<OnboardSectorWrite, OnboardWriteError> {
    let (info, sector) = writable_active_sector(feature).await?;
    if backup.len() != usize::from(info.sector_size) || !layout::sector_crc_valid(&backup) {
        return Err(OnboardWriteError::InvalidBackup {
            expected: info.sector_size,
        });
    }
    let fail =
        |e: Hidpp20Error| classify_hidpp_error(e, HidppOperation::WriteOnboardButton, FEATURE);
    let before = feature
        .read_sector(sector, info.sector_size)
        .await
        .map_err(fail)?;
    write_verified(feature, sector, before, backup, info.sector_size).await
}

mod buttons;
mod macros;

pub use buttons::{
    get_onboard_buttons, get_onboard_buttons_on, set_onboard_button, set_onboard_button_on,
};

#[cfg(test)]
mod tests;
