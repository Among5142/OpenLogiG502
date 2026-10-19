use std::sync::Arc;

use hidpp::{
    channel::HidppChannel,
    device::Device,
    feature::CreatableFeature,
    feature::FeatureType,
    feature::adc_measurement::AdcMeasurementFeature,
    feature::battery_status::BatteryStatusFeature,
    feature::device_information::{
        DeviceEntityFirmwareInfo, DeviceEntityType, DeviceInformationFeature,
    },
    feature::feature_set::FeatureSetFeature,
    feature::onboard_profiles::{
        OnboardMode, OnboardProfilesFeature, OnboardProfilesInfo,
        layout::{
            self, DirectoryEntry, LayoutError, ProfileSector, ROM_SECTOR_BASE,
            USER_DIRECTORY_SECTOR,
        },
    },
    feature::unified_battery::UnifiedBatteryFeature,
    protocol::v20::Hidpp20Error,
};

use crate::backend::HidBackend;
use crate::channel::route::DeviceRoute;
use crate::reprog_controls::{self, CidFlags, CidInfo, ReprogControlsV4};
use crate::write::{HidppOperation, WriteError, classify_hidpp_error, open_feature, with_route};

/// Snapshot of one HID++ feature exposed by a device: protocol ID +
/// version. Returned by [`dump_features`] for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureEntry {
    /// HID++ feature ID.
    pub id: u16,
    /// Feature version reported by the device.
    pub version: u8,
    /// Obsolete / hidden / engineering flags the device advertises alongside
    /// the feature.
    pub typ: FeatureType,
}

/// Snapshot of one HID++ `0x1b04` reprogrammable control. Returned by
/// [`dump_reprog_controls`] for diagnostics so new device controls can be
/// identified before OpenLogi maps them to a first-class button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReprogControlEntry {
    /// HID++ control ID.
    pub cid: u16,
    /// Default task ID assigned to the control.
    pub task_id: u16,
    /// Capability and classification flags for the control.
    pub flags: CidFlags,
}

impl From<CidInfo> for ReprogControlEntry {
    fn from(info: CidInfo) -> Self {
        Self {
            cid: info.cid.into(),
            task_id: info.task_id.0,
            flags: info.flags,
        }
    }
}

/// Enumerate every HID++ feature the device on `route` reports — used by
/// `openlogi diag features` to confirm which DPI / SmartShift / etc.
/// feature IDs a given peripheral actually exposes (e.g. whether a mouse
/// speaks `0x2201 AdjustableDpi`, `0x2202 ExtendedAdjustableDpi`, or both —
/// `write::dpi` drives either).
pub async fn dump_features(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<Vec<FeatureEntry>, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        // The root feature exposes the FeatureSet (0x0001) at a fixed
        // address; we look it up directly rather than going through
        // `enumerate_features` so the iteration is observable.
        let feature_set_info = device
            .root()
            .get_feature(FeatureSetFeature::ID)
            .await
            .map_err(|e| {
                classify_hidpp_error(e, HidppOperation::DumpFeatures, FeatureSetFeature::ID)
            })?
            .ok_or(WriteError::FeatureUnsupported {
                feature_hex: FeatureSetFeature::ID,
            })?;
        let feature_set = device.add_feature::<FeatureSetFeature>(feature_set_info.index);
        let count = feature_set.count().await.map_err(|e| {
            classify_hidpp_error(e, HidppOperation::DumpFeatures, FeatureSetFeature::ID)
        })?;
        let mut entries = Vec::with_capacity(usize::from(count));
        for i in 0..=count {
            let info = feature_set.get_feature(i).await.map_err(|e| {
                classify_hidpp_error(e, HidppOperation::DumpFeatures, FeatureSetFeature::ID)
            })?;
            entries.push(FeatureEntry {
                id: info.id,
                version: info.version,
                typ: info.typ,
            });
        }
        Ok(entries)
    })
    .await
}

/// Enumerate the device's HID++ `0x1b04` reprogrammable controls. This is a
/// diagnostics-only probe used to discover controls for newly released devices.
/// For example, MX Master 4 has both a Gesture Button and a separate Haptic
/// Sense Panel in the thumb area; this probe lets us identify the panel's CID
/// and capabilities before wiring it into the capture/remapping model.
pub async fn dump_reprog_controls(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<Vec<ReprogControlEntry>, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let info = device
            .root()
            .get_feature(reprog_controls::FEATURE_ID)
            .await
            .map_err(|e| {
                classify_hidpp_error(e, HidppOperation::DumpFeatures, reprog_controls::FEATURE_ID)
            })?
            .ok_or(WriteError::FeatureUnsupported {
                feature_hex: reprog_controls::FEATURE_ID,
            })?;
        let rc = ReprogControlsV4::new(Arc::clone(&channel), index, info.index);
        let count = rc.get_count().await.map_err(|e| {
            classify_hidpp_error(e, HidppOperation::DumpFeatures, reprog_controls::FEATURE_ID)
        })?;
        let mut entries = Vec::with_capacity(usize::from(count));
        for i in 0..count {
            let control = rc.get_cid_info(i).await.map_err(|e| {
                classify_hidpp_error(e, HidppOperation::DumpFeatures, reprog_controls::FEATURE_ID)
            })?;
            entries.push(control.into());
        }
        Ok(entries)
    })
    .await
}

/// Everything `openlogi diag profiles` reads from a `0x8100` profile store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardProfilesDump {
    /// The store description.
    pub info: OnboardProfilesInfo,
    /// Whether onboard profiles or the host drive the buttons.
    pub mode: OnboardMode,
    /// Sector of the active profile, as reported.
    pub current_sector: u16,
    /// The user directory, or why it could not be used. `None` when the
    /// store's layout is not one this build decodes, so nothing past
    /// `getInfo` was read.
    pub directory: Option<DirectoryDump>,
    /// Every user profile the directory lists, then every ROM profile.
    pub sectors: Vec<SectorDump>,
}

/// The user profile directory as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryDump {
    /// Whether the directory sector's CRC matched. Firmware falls back to its
    /// ROM profiles when it does not.
    pub crc_valid: bool,
    /// The decoded entries.
    pub entries: Result<Vec<DirectoryEntry>, LayoutError>,
}

/// What a profile sector's trailing checksum says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorCrc {
    /// The stored CRC matches the contents.
    Valid,
    /// The stored CRC does not match; firmware ignores such a user sector.
    Mismatch,
    /// A factory (ROM) sector. ROM sectors store `00 00` where user sectors
    /// keep their CRC (seen on a G502 X LIGHTSPEED), and libratbag reads
    /// them without checking, so nothing is compared.
    NotStored,
}

/// One profile sector as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectorDump {
    /// The sector address.
    pub sector: u16,
    /// Whether this is a factory (ROM) profile rather than a user one.
    pub rom: bool,
    /// The directory's enabled flag; `None` for ROM sectors.
    pub enabled: Option<bool>,
    /// The sector's checksum state.
    pub crc: SectorCrc,
    /// The decoded profile.
    pub profile: Result<ProfileSector, LayoutError>,
    /// The sector bytes exactly as read, for checking the decoder.
    pub raw: Vec<u8>,
}

/// Read the `0x8100` onboard profile store of the device on `route`: the
/// description, mode, active sector, user directory, and every user and ROM
/// profile sector. Read-only — for `openlogi diag profiles`, so the
/// reverse-engineered layout can be checked against real hardware.
pub async fn dump_onboard_profiles(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<OnboardProfilesDump, WriteError> {
    const FEATURE: u16 = <OnboardProfilesFeature as CreatableFeature>::ID;
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let feature = open_feature::<OnboardProfilesFeature>(&mut device).await?;
        let fail = |e| classify_hidpp_error(e, HidppOperation::DumpFeatures, FEATURE);

        let info = feature.get_info().await.map_err(fail)?;
        let mode = feature.get_onboard_mode().await.map_err(fail)?;
        let current_sector = feature.get_current_profile_sector().await.map_err(fail)?;
        let mut dump = OnboardProfilesDump {
            info,
            mode,
            current_sector,
            directory: None,
            sectors: Vec::new(),
        };
        if !info.layout_supported() {
            return Ok(dump);
        }

        let read = |sector| feature.read_sector(sector, info.sector_size);
        let raw_directory = read(USER_DIRECTORY_SECTOR).await.map_err(fail)?;
        let directory = DirectoryDump {
            crc_valid: layout::sector_crc_valid(&raw_directory),
            entries: layout::decode_directory(&raw_directory),
        };
        let user_sectors: Vec<(u16, Option<bool>)> = match &directory.entries {
            Ok(entries) if directory.crc_valid => entries
                .iter()
                .map(|entry| (entry.sector, Some(entry.enabled)))
                .collect(),
            _ => Vec::new(),
        };
        let rom_sectors =
            (1..=u16::from(info.rom_profile_count)).map(|n| (ROM_SECTOR_BASE + n, None));
        for (sector, enabled) in user_sectors.into_iter().chain(rom_sectors) {
            let raw = read(sector).await.map_err(fail)?;
            dump.sectors.push(SectorDump {
                sector,
                rom: enabled.is_none(),
                enabled,
                crc: match (enabled, layout::sector_crc_valid(&raw)) {
                    (None, _) => SectorCrc::NotStored,
                    (Some(_), true) => SectorCrc::Valid,
                    (Some(_), false) => SectorCrc::Mismatch,
                },
                profile: ProfileSector::decode(&raw),
                raw,
            });
        }
        dump.directory = Some(directory);
        Ok(dump)
    })
    .await
}

/// Diagnostic read of the device's raw battery report — the unified `0x1004`
/// fields, the legacy `0x1000` `discharge_level`/`next_level`/`status`, or the
/// `0x1F20` ADC voltage and charging state. For
/// `openlogi diag battery`: surfaces exactly what the firmware reports so a
/// claim like "MX2S shows 0% while charging" can be confirmed against the wire
/// instead of guessed (the GUI only ever shows the mapped value).
pub async fn read_battery_raw(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<String, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;

        match open_feature::<UnifiedBatteryFeature>(&mut device).await {
            Ok(feature) => {
                let info = feature
                    .get_battery_info()
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
                return Ok(format!(
                    "0x1004 UnifiedBattery: percentage={} level={:?} status={:?}",
                    info.charging_percentage, info.level, info.status
                ));
            }
            Err(WriteError::FeatureUnsupported { .. }) => {}
            Err(e) => return Err(e),
        }

        match open_feature::<BatteryStatusFeature>(&mut device).await {
            Ok(feature) => {
                let info = feature
                    .get_battery_level_status()
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
                return Ok(format!(
                    "0x1000 BatteryStatus: discharge_level={} next_level={} status={:?}",
                    info.discharge_level, info.next_level, info.status
                ));
            }
            Err(WriteError::FeatureUnsupported { .. }) => {}
            Err(e) => return Err(e),
        }

        match open_feature::<AdcMeasurementFeature>(&mut device).await {
            Ok(feature) => {
                let measurement = feature
                    .get_adc_measurement()
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
                return Ok(format!("0x1F20 AdcMeasurement: {measurement:?}"));
            }
            Err(WriteError::FeatureUnsupported { .. }) => {}
            Err(e) => return Err(e),
        }

        // Reached only when none of the read features is present; report the
        // preferred feature rather than implying 0x1F20 was specifically absent.
        Err(WriteError::FeatureUnsupported {
            feature_hex: 0x1004,
        })
    })
    .await
}

/// Firmware fields for one entity whose record the device answered and this
/// parser decoded.
///
/// Owned, constructible data converted from `hidpp`'s
/// `DeviceEntityFirmwareInfo`, the same way [`ReprogControlEntry`] is
/// converted from `CidInfo`: consumers get the structured record and decide
/// how to render it, rather than being handed a pre-formatted string with the
/// rest of the fields dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareEntityInfo {
    /// What the entity is: main application, bootloader, radio stack, and so
    /// on.
    pub kind: DeviceEntityType,
    /// Three-letter prefix of the firmware name, e.g. `MPM`.
    pub prefix: String,
    /// Firmware number, decoded by the protocol layer from packed BCD, or from
    /// binary when the firmware sends binary.
    pub number: u8,
    /// Firmware revision, decoded by the protocol layer from packed BCD, or from
    /// binary when the firmware sends binary.
    pub revision: u8,
    /// Firmware build, decoded by the protocol layer from packed BCD, or from
    /// binary when the firmware sends binary.
    pub build: u16,
    /// Whether this is the entity currently running.
    pub active: bool,
    /// USB or wireless product ID the entity runs under. A bootloader entity
    /// reports the PID the device enumerates as while in DFU mode; only the
    /// active entity is required to report a real value, so an inactive one
    /// may be zero.
    pub transport_pid: u16,
    /// Optional extra versioning bytes. Device-specific and usually all zero,
    /// carried verbatim because a device that does populate them is exactly
    /// the device a report is being collected for.
    pub extra_version: [u8; 5],
}

impl From<DeviceEntityFirmwareInfo> for FirmwareEntityInfo {
    fn from(info: DeviceEntityFirmwareInfo) -> Self {
        Self {
            kind: info.entity_type,
            prefix: info.firmware_prefix,
            number: info.firmware_number,
            revision: info.revision,
            build: info.build,
            active: info.active,
            transport_pid: info.transport_pid,
            extra_version: info.extra_version,
        }
    }
}

/// One firmware entity a device reports through HID++ `0x0003` function 1.
/// Returned by [`dump_firmware_entities`] so a device report can name the
/// exact firmware it is running.
///
/// There are two states and only two: the device answered with a record that
/// decoded, or it did not. An enum makes "a version with no kind" and "an
/// error alongside a version" unrepresentable rather than merely unreachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareEntity {
    /// The entity's record was read and decoded.
    Readable {
        /// Index of the entity in the device's own table.
        index: u8,
        /// The decoded firmware record.
        info: FirmwareEntityInfo,
    },
    /// The device declared the entity, but its record could not be read.
    ///
    /// Reported rather than dropped: omitting the row would claim the device
    /// has fewer firmware images than it says it has, and a device that cannot
    /// describe one of its own images is what a bug report needs to say.
    Unreadable {
        /// Index of the entity in the device's own table.
        index: u8,
        /// Why the record could not be read.
        error: WriteError,
    },
}

/// Read every firmware entity the device on `route` reports.
///
/// A device lists its main application firmware alongside its bootloader and,
/// on many models, a separate radio stack. `openlogi diag features` prints
/// them so a bug report names the firmware that produced the behaviour rather
/// than just the model.
///
/// A single entity the *device* declined or answered unparseably does not fail
/// the call — see [`FirmwareEntity::Unreadable`]. A channel failure does: the
/// route is gone, not the entity.
pub async fn dump_firmware_entities(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<Vec<FirmwareEntity>, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        dump_firmware_entities_on_channel(&channel, index).await
    })
    .await
}

/// [`dump_firmware_entities`] against an already-open channel, the shape the
/// tests drive a scripted device through.
pub(crate) async fn dump_firmware_entities_on_channel(
    channel: &Arc<HidppChannel>,
    index: u8,
) -> Result<Vec<FirmwareEntity>, WriteError> {
    let mut device = Device::new(Arc::clone(channel), index)
        .await
        .map_err(|_| WriteError::DeviceUnreachable { index })?;
    let feature = open_feature::<DeviceInformationFeature>(&mut device).await?;
    let info = feature.get_device_info().await.map_err(|e| {
        classify_hidpp_error(
            e,
            HidppOperation::DumpFeatures,
            DeviceInformationFeature::ID,
        )
    })?;

    let mut entries = Vec::with_capacity(usize::from(info.entity_count));
    for entity in 0..info.entity_count {
        match feature.get_fw_info(entity).await {
            Ok(fw) => entries.push(FirmwareEntity::Readable {
                index: entity,
                info: fw.into(),
            }),
            // The device answered about *this* entity and the answer was no:
            // it refused the read, or it sent a record this parser cannot
            // decode (a firmware prefix that is not UTF-8). The rest of the
            // table is still worth reading.
            Err(e @ (Hidpp20Error::Feature(_) | Hidpp20Error::UnsupportedResponse)) => {
                entries.push(FirmwareEntity::Unreadable {
                    index: entity,
                    error: classify_hidpp_error(
                        e,
                        HidppOperation::DumpFeatures,
                        DeviceInformationFeature::ID,
                    ),
                });
            }
            // A channel failure says nothing about the entity — the route
            // disappeared. Carrying on would spend a timeout per remaining
            // entity and then print malformed-firmware rows for a disconnect.
            Err(e) => {
                return Err(classify_hidpp_error(
                    e,
                    HidppOperation::DumpFeatures,
                    DeviceInformationFeature::ID,
                ));
            }
        }
    }
    Ok(entries)
}
