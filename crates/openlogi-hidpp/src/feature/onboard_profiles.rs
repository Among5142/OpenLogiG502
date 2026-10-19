//! Implements `OnboardProfiles` (feature `0x8100`): the on-device profile
//! store G-series mice keep their DPI slots, report rate and button bindings
//! in. Mice that expose this instead of `0x1b04` (the G502 X LIGHTSPEED,
//! among others) bind their extra buttons here.
//!
//! **Reverse-engineered.** The function ids and request/response layouts
//! below were cross-checked between libratbag (`src/hidpp20.c`,
//! `CMD_ONBOARD_PROFILES_*`) and Solaar (`lib/logitech_receiver/hidpp20.py`,
//! `OnboardProfiles`); Logitech's public spec drop does not cover `0x8100`.
//! The sector layout lives in [`layout`].
//!
//! Only reads are implemented. Writing a sector rewrites device flash.

use num_enum::TryFromPrimitive;
use openlogi_hidpp_derive::Feature;

use crate::{feature::FeatureEndpoint, protocol::v20::Hidpp20Error};

pub mod layout;

/// Bytes returned by one `memoryRead`.
pub const MEMORY_READ_CHUNK: u16 = 16;

/// `getInfo` function id.
const FN_GET_INFO: u8 = 0x0;
/// `getOnboardMode` function id.
const FN_GET_ONBOARD_MODE: u8 = 0x2;
/// `getCurrentProfile` function id.
const FN_GET_CURRENT_PROFILE: u8 = 0x4;
/// `memoryRead` function id.
const FN_MEMORY_READ: u8 = 0x5;

/// The memory model both references support (libratbag
/// `HIDPP20_ONBOARD_PROFILES_MEMORY_TYPE_G402`).
pub const SUPPORTED_MEMORY_MODEL: u8 = 0x01;

/// Profile formats whose layout [`layout::ProfileSector`] decodes: `0x01`
/// (G402) through `0x05` (G502 X). libratbag validates the same range;
/// Solaar refuses anything above `0x05`.
pub const SUPPORTED_PROFILE_FORMATS: std::ops::RangeInclusive<u8> = 0x01..=0x05;

/// The macro format both references support.
pub const SUPPORTED_MACRO_FORMAT: u8 = 0x01;

/// The mechanical-layout byte from `getInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MechanicalLayout(pub u8);

impl MechanicalLayout {
    /// Whether profiles carry a G-shift binding layer (bits 0–1 equal `0b10`,
    /// as both references test it).
    #[must_use]
    pub fn has_gshift(self) -> bool {
        self.0 & 0x03 == 0x02
    }

    /// Whether the mouse has a DPI-shift button (bits 2–3 equal `0b10`, per
    /// libratbag; Solaar does not read these bits).
    #[must_use]
    pub fn has_dpi_shift(self) -> bool {
        (self.0 >> 2) & 0x03 == 0x02
    }
}

/// What `getInfo` reports about the profile store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct OnboardProfilesInfo {
    /// Memory model; only [`SUPPORTED_MEMORY_MODEL`] is understood.
    pub memory_model: u8,
    /// Profile sector format; see [`SUPPORTED_PROFILE_FORMATS`].
    pub profile_format: u8,
    /// Macro format; only [`SUPPORTED_MACRO_FORMAT`] is understood.
    pub macro_format: u8,
    /// Number of user profile slots.
    pub profile_count: u8,
    /// Number of factory (ROM) profiles.
    pub rom_profile_count: u8,
    /// Number of physical buttons with a binding slot.
    pub button_count: u8,
    /// Number of writable sectors.
    pub sector_count: u8,
    /// Size of every sector in bytes, CRC included.
    pub sector_size: u16,
    /// Which layered bindings the mouse supports.
    pub mechanical_layout: MechanicalLayout,
    /// Raw `various info` byte; bits 0–2 encode corded/wireless per
    /// libratbag, the rest is undocumented.
    pub various_info: u8,
}

impl OnboardProfilesInfo {
    /// Whether this store uses a memory model, profile format and macro
    /// format whose layout this crate decodes. Reading sectors of any other
    /// format would misinterpret them, and writing them would corrupt them.
    #[must_use]
    pub fn layout_supported(&self) -> bool {
        self.memory_model == SUPPORTED_MEMORY_MODEL
            && SUPPORTED_PROFILE_FORMATS.contains(&self.profile_format)
            && self.macro_format == SUPPORTED_MACRO_FORMAT
    }
}

/// Who decides what the buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, TryFromPrimitive)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[repr(u8)]
pub enum OnboardMode {
    /// The active onboard profile applies.
    Onboard = 0x01,
    /// The host drives the device; onboard profiles are suspended.
    Host = 0x02,
}

/// Implements the `OnboardProfiles` / `0x8100` feature.
#[derive(Clone, Feature)]
#[creatable(id = 0x8100, version = 0)]
pub struct OnboardProfilesFeature {
    /// The endpoint this feature talks to.
    endpoint: FeatureEndpoint,
}

impl OnboardProfilesFeature {
    /// Reads the profile-store description.
    pub async fn get_info(&self) -> Result<OnboardProfilesInfo, Hidpp20Error> {
        let p = self
            .endpoint
            .call(FN_GET_INFO, [0; 3])
            .await?
            .extend_payload();
        Ok(OnboardProfilesInfo {
            memory_model: p[0],
            profile_format: p[1],
            macro_format: p[2],
            profile_count: p[3],
            rom_profile_count: p[4],
            button_count: p[5],
            sector_count: p[6],
            sector_size: u16::from_be_bytes([p[7], p[8]]),
            mechanical_layout: MechanicalLayout(p[9]),
            various_info: p[10],
        })
    }

    /// Reads whether onboard profiles or the host are in control. A mode
    /// byte outside [`OnboardMode`] fails with
    /// [`Hidpp20Error::UnsupportedResponse`].
    pub async fn get_onboard_mode(&self) -> Result<OnboardMode, Hidpp20Error> {
        let mode = self
            .endpoint
            .call(FN_GET_ONBOARD_MODE, [0; 3])
            .await?
            .extend_payload()[0];
        OnboardMode::try_from(mode).map_err(|_| Hidpp20Error::UnsupportedResponse)
    }

    /// Reads the sector of the active profile.
    ///
    /// The reply's first two bytes are read here as one big-endian sector
    /// address. That reading is inferred, not documented: libratbag takes
    /// byte 1 alone as the profile index and needs an `INDEX_OFFSET` quirk
    /// for the G502 X because it answers `1` for the first profile — which
    /// is exactly user sector `0x0001`. Callers match the result against
    /// [`layout::DirectoryEntry::sector`].
    pub async fn get_current_profile_sector(&self) -> Result<u16, Hidpp20Error> {
        let p = self
            .endpoint
            .call(FN_GET_CURRENT_PROFILE, [0; 3])
            .await?
            .extend_payload();
        Ok(u16::from_be_bytes([p[0], p[1]]))
    }

    /// Reads 16 bytes of `sector` starting at `offset`.
    ///
    /// The firmware rejects an `offset` past `sector_size - 16` with
    /// `InvalidArgument`; [`Self::read_sector`] handles the tail.
    pub async fn read_memory(
        &self,
        sector: u16,
        offset: u16,
    ) -> Result<[u8; MEMORY_READ_CHUNK as usize], Hidpp20Error> {
        let mut args = [0; 16];
        args[..2].copy_from_slice(&sector.to_be_bytes());
        args[2..4].copy_from_slice(&offset.to_be_bytes());
        Ok(self
            .endpoint
            .call_long(FN_MEMORY_READ, args)
            .await?
            .extend_payload())
    }

    /// Reads a whole sector of `sector_size` bytes (from
    /// [`OnboardProfilesInfo::sector_size`]).
    ///
    /// Reads in 16-byte chunks. When the size is not a multiple of 16 the
    /// final chunk is read from `sector_size - 16` and only its unread tail
    /// kept, the way both references do it. A size under 16 cannot be read
    /// and fails with [`Hidpp20Error::UnsupportedResponse`] without sending
    /// anything.
    pub async fn read_sector(
        &self,
        sector: u16,
        sector_size: u16,
    ) -> Result<Vec<u8>, Hidpp20Error> {
        let last = sector_size
            .checked_sub(MEMORY_READ_CHUNK)
            .ok_or(Hidpp20Error::UnsupportedResponse)?;
        let mut data = Vec::with_capacity(usize::from(sector_size));
        let mut offset = 0;
        while offset < sector_size {
            let at = offset.min(last);
            let chunk = self.read_memory(sector, at).await?;
            // `offset - at` is non-zero only on the tail read, where it skips
            // bytes the previous chunk already delivered.
            data.extend_from_slice(&chunk[usize::from(offset - at)..]);
            offset = at + MEMORY_READ_CHUNK;
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use std::{assert_matches, sync::Arc};

    use super::{MechanicalLayout, OnboardMode, OnboardProfilesFeature};
    use crate::{
        channel::mock::{MockRawHidChannel, MockRawHidHandle, channel_with_reader},
        feature::CreatableFeature,
        nibble::U4,
        protocol::v20::{Hidpp20Error, Message, MessageHeader},
    };

    const DEVICE: u8 = 1;
    const FEATURE_INDEX: u8 = 9;

    fn header(function: u8) -> MessageHeader {
        MessageHeader {
            device_index: DEVICE,
            feature_index: FEATURE_INDEX,
            function_id: U4::from_lo(function),
            software_id: U4::from_lo(1),
        }
    }

    async fn feature() -> (OnboardProfilesFeature, MockRawHidHandle) {
        let (raw, handle) = MockRawHidChannel::new();
        let channel = Arc::new(channel_with_reader(raw).await);
        (
            OnboardProfilesFeature::new(channel, DEVICE, FEATURE_INDEX),
            handle,
        )
    }

    #[test]
    fn get_info_decodes_a_g502x_style_reply() {
        futures::executor::block_on(async {
            let (feature, handle) = feature().await;
            let mut pending = Box::pin(feature.get_info());
            assert!(futures::poll!(pending.as_mut()).is_pending());
            let mut reply = [0; 16];
            reply[..11].copy_from_slice(&[1, 5, 1, 3, 1, 13, 16, 0x00, 0xff, 0x0a, 0x02]);
            handle
                .send_incoming(Message::Long(header(0), reply).into())
                .await;
            let info = pending.await.unwrap();
            assert_eq!(info.profile_format, 5);
            assert_eq!(info.button_count, 13);
            assert_eq!(info.sector_size, 255);
            assert_eq!(info.mechanical_layout, MechanicalLayout(0x0a));
            assert!(info.mechanical_layout.has_gshift());
            assert!(info.mechanical_layout.has_dpi_shift());
            assert!(info.layout_supported());
        });
    }

    #[test]
    fn an_unknown_onboard_mode_is_an_error() {
        for (byte, expected) in [
            (1, Some(OnboardMode::Onboard)),
            (2, Some(OnboardMode::Host)),
            (0, None),
            (3, None),
        ] {
            let got = futures::executor::block_on(async {
                let (feature, handle) = feature().await;
                let mut pending = Box::pin(feature.get_onboard_mode());
                assert!(futures::poll!(pending.as_mut()).is_pending());
                handle
                    .send_incoming(Message::Short(header(2), [byte, 0, 0]).into())
                    .await;
                pending.await
            });
            match expected {
                Some(mode) => assert_eq!(got.unwrap(), mode),
                None => assert_matches!(got, Err(Hidpp20Error::UnsupportedResponse)),
            }
        }
    }

    /// Reads a 40-byte sector against a device whose memory byte at offset
    /// *n* is *n*, and checks the request offsets the reader sent.
    #[test]
    fn read_sector_reads_the_tail_from_size_minus_sixteen() {
        futures::executor::block_on(async {
            let (feature, handle) = feature().await;
            let mut pending = Box::pin(feature.read_sector(0x0001, 40));
            for (sent, expected_offset) in [0u8, 16, 24].into_iter().enumerate() {
                // The previous reply reaches the reader asynchronously; poll
                // until this chunk's request has been written.
                for _ in 0..50 {
                    assert!(futures::poll!(pending.as_mut()).is_pending());
                    if handle.written_reports().len() > sent {
                        break;
                    }
                    futures_timer::Delay::new(std::time::Duration::from_millis(2)).await;
                }
                let written = handle.written_reports();
                assert_eq!(written.len(), sent + 1, "chunk request {sent} never sent");
                let request = written[sent].clone();
                // report id, device, feature, fn|sw, then sector BE, offset BE.
                assert_eq!(&request[4..8], &[0x00, 0x01, 0x00, expected_offset]);
                let chunk: [u8; 16] =
                    std::array::from_fn(|i| expected_offset + u8::try_from(i).unwrap());
                // Each request carries the channel's next software id; answer
                // with the request's own header so the reply matches it.
                let reply = MessageHeader {
                    software_id: U4::from_lo(request[3] & 0x0f),
                    ..header(5)
                };
                handle
                    .send_incoming(Message::Long(reply, chunk).into())
                    .await;
            }
            let sector = pending.await.unwrap();
            let expected: Vec<u8> = (0..40).collect();
            assert_eq!(sector, expected);
        });
    }

    #[test]
    fn a_sector_smaller_than_one_chunk_is_refused_unsent() {
        futures::executor::block_on(async {
            let (feature, handle) = feature().await;
            assert_matches!(
                feature.read_sector(0, 8).await,
                Err(Hidpp20Error::UnsupportedResponse)
            );
            assert!(handle.written_reports().is_empty());
        });
    }
}
