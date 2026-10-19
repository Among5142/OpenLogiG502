//! Remap and restore against a scripted G502 X whose `0x8100` flash is an
//! in-memory sector map: reads serve it, the write sequence commits to it.

use std::assert_matches;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hidpp::feature::CreatableFeature;
use hidpp::feature::onboard_profiles::OnboardProfilesFeature;
use hidpp::feature::onboard_profiles::layout::{
    self, BindingLayer, ButtonBinding, ButtonFunction, KeyModifiers, MouseButtons,
};

use super::{OnboardBindingChange, OnboardWriteError, remap_on, restore_on};
use crate::channel::scripted::{ScriptedRawHidChannel, scripted_channel};

/// The `0x8100` feature index on the scripted mouse.
const INDEX: u8 = 0x09;
/// The mouse's HID++ device index.
const DEVICE: u8 = 0xff;
/// The G502 X LIGHTSPEED's sector size.
const SECTOR_SIZE: usize = 255;

/// A mouse with a `0x8100` store shaped like a G502 X LIGHTSPEED:
/// profile format `0x03`, 11 buttons, 16 sectors of 255 bytes.
struct Mouse {
    sectors: HashMap<u16, Vec<u8>>,
    /// The sector `getCurrentProfile` reports.
    active: u16,
    /// Whether `memoryWriteEnd` commits (`false` drops every write).
    commits: bool,
    /// The open write: sector, declared length, bytes so far.
    open: Option<(u16, usize, Vec<u8>)>,
    /// Sectors committed, in order.
    committed: Vec<u16>,
}

fn sealed(mut sector: Vec<u8>) -> Vec<u8> {
    layout::seal(&mut sector);
    sector
}

/// A sealed user profile with the G502 X factory bindings.
fn stock_profile() -> Vec<u8> {
    let mut sector = vec![0xff; SECTOR_SIZE];
    let bindings = [
        ButtonBinding::Mouse(MouseButtons::LEFT),
        ButtonBinding::Mouse(MouseButtons::RIGHT),
        ButtonBinding::Mouse(MouseButtons::MIDDLE),
        ButtonBinding::Mouse(MouseButtons::BACK),
        function(ButtonFunction::ShiftDpi),
        ButtonBinding::Mouse(MouseButtons::FORWARD),
        function(ButtonFunction::TiltLeft),
        function(ButtonFunction::TiltRight),
        function(ButtonFunction::CycleProfile),
        function(ButtonFunction::NextDpi),
        function(ButtonFunction::PreviousDpi),
    ];
    for (slot, binding) in bindings.into_iter().enumerate() {
        layout::set_binding(&mut sector, BindingLayer::Primary, slot, binding).unwrap();
    }
    sealed(sector)
}

fn function(function: ButtonFunction) -> ButtonBinding {
    ButtonBinding::Function { function, data: 0 }
}

/// A sealed directory listing user sectors 1 and 2.
fn directory() -> Vec<u8> {
    let mut sector = vec![0xff; SECTOR_SIZE];
    sector[..12].copy_from_slice(&[0, 1, 1, 0, 0, 2, 1, 0, 0xff, 0xff, 0, 0]);
    sealed(sector)
}

impl Mouse {
    fn g502x() -> Self {
        let mut sectors = HashMap::new();
        sectors.insert(0x0000, directory());
        sectors.insert(0x0001, stock_profile());
        sectors.insert(0x0002, stock_profile());
        // ROM sectors carry no CRC.
        let mut rom = stock_profile();
        rom[SECTOR_SIZE - 2..].fill(0);
        sectors.insert(0x0101, rom);
        Self {
            sectors,
            active: 0x0001,
            commits: true,
            open: None,
            committed: Vec::new(),
        }
    }

    fn respond(&mut self, request: &[u8]) -> Option<Vec<u8>> {
        if request.len() < 7 || request[2] != INDEX {
            return None;
        }
        let args = &request[4..];
        let mut payload = [0u8; 16];
        match request[3] >> 4 {
            0x0 => payload[..11].copy_from_slice(&[1, 3, 1, 5, 2, 11, 16, 0x00, 0xff, 0x0a, 0x04]),
            0x2 => payload[0] = 1,
            0x4 => payload[..2].copy_from_slice(&self.active.to_be_bytes()),
            0x5 => {
                let sector = u16::from_be_bytes([args[0], args[1]]);
                let offset = usize::from(u16::from_be_bytes([args[2], args[3]]));
                let data = self.sectors.get(&sector)?;
                payload.copy_from_slice(&data[offset..offset + 16]);
            }
            0x6 => {
                let sector = u16::from_be_bytes([args[0], args[1]]);
                let length = usize::from(u16::from_be_bytes([args[4], args[5]]));
                self.open = Some((sector, length, Vec::new()));
            }
            0x7 => self.open.as_mut()?.2.extend_from_slice(&args[..16]),
            0x8 => {
                let (sector, length, mut data) = self.open.take()?;
                data.truncate(length);
                if self.commits {
                    self.sectors.insert(sector, data);
                }
                self.committed.push(sector);
            }
            _ => return None,
        }
        let mut response = vec![0u8; 20];
        response[0] = 0x11;
        response[1..4].copy_from_slice(&request[1..4]);
        response[4..].copy_from_slice(&payload);
        Some(response)
    }
}

async fn feature(mouse: &Arc<Mutex<Mouse>>) -> OnboardProfilesFeature {
    let mouse = Arc::clone(mouse);
    let (raw, _handle) = ScriptedRawHidChannel::with_dynamic_responder(move |request| {
        mouse.lock().expect("mouse lock").respond(request)
    });
    OnboardProfilesFeature::new(scripted_channel(raw).await, DEVICE, INDEX)
}

fn g8_to_key_a() -> OnboardBindingChange {
    OnboardBindingChange {
        layer: BindingLayer::Primary,
        slot: 9,
        binding: ButtonBinding::Key {
            modifiers: KeyModifiers::empty(),
            usage: 0x04,
        },
    }
}

#[tokio::test]
async fn a_remap_rewrites_only_its_slot_and_verifies() {
    let mouse = Arc::new(Mutex::new(Mouse::g502x()));
    let feature = feature(&mouse).await;

    let outcome = remap_on(&feature, &[g8_to_key_a()]).await.unwrap();

    assert!(outcome.changed());
    assert_eq!(outcome.sector, 0x0001);
    assert_eq!(outcome.before, stock_profile());
    let mouse = mouse.lock().unwrap();
    assert_eq!(mouse.committed, [0x0001]);
    let flash = &mouse.sectors[&0x0001];
    assert_eq!(flash, &outcome.after);
    assert!(layout::sector_crc_valid(flash));
    let changed: Vec<usize> = (0..SECTOR_SIZE)
        .filter(|&i| flash[i] != outcome.before[i])
        .collect();
    assert!(
        changed
            .iter()
            .all(|&i| (0x44..0x48).contains(&i) || i >= 253),
        "{changed:x?}"
    );
    // The other user profile is untouched.
    assert_eq!(mouse.sectors[&0x0002], stock_profile());
}

#[tokio::test]
async fn an_unchanged_binding_writes_nothing() {
    let mouse = Arc::new(Mutex::new(Mouse::g502x()));
    let feature = feature(&mouse).await;
    let same = OnboardBindingChange {
        binding: function(ButtonFunction::NextDpi),
        ..g8_to_key_a()
    };

    let outcome = remap_on(&feature, &[same]).await.unwrap();

    assert!(!outcome.changed());
    assert!(mouse.lock().unwrap().committed.is_empty());
}

#[tokio::test]
async fn a_factory_profile_is_never_written() {
    let mouse = Arc::new(Mutex::new(Mouse {
        active: 0x0101,
        ..Mouse::g502x()
    }));
    let feature = feature(&mouse).await;

    assert_matches!(
        remap_on(&feature, &[g8_to_key_a()]).await,
        Err(OnboardWriteError::ActiveProfileNotWritable(0x0101))
    );
    assert!(mouse.lock().unwrap().committed.is_empty());
}

#[tokio::test]
async fn a_sector_missing_from_the_directory_is_never_written() {
    let mouse = Arc::new(Mutex::new(Mouse {
        active: 0x0003,
        ..Mouse::g502x()
    }));
    let feature = feature(&mouse).await;

    assert_matches!(
        remap_on(&feature, &[g8_to_key_a()]).await,
        Err(OnboardWriteError::ActiveProfileNotWritable(0x0003))
    );
    assert!(mouse.lock().unwrap().committed.is_empty());
}

#[tokio::test]
async fn an_invalid_directory_blocks_every_write() {
    let mut state = Mouse::g502x();
    state.sectors.get_mut(&0x0000).unwrap()[100] ^= 1;
    let mouse = Arc::new(Mutex::new(state));
    let feature = feature(&mouse).await;

    assert_matches!(
        remap_on(&feature, &[g8_to_key_a()]).await,
        Err(OnboardWriteError::DirectoryInvalid)
    );
    assert!(mouse.lock().unwrap().committed.is_empty());
}

#[tokio::test]
async fn a_corrupt_active_profile_is_not_resealed() {
    let mut state = Mouse::g502x();
    state.sectors.get_mut(&0x0001).unwrap()[0x10] ^= 1;
    let mouse = Arc::new(Mutex::new(state));
    let feature = feature(&mouse).await;

    assert_matches!(
        remap_on(&feature, &[g8_to_key_a()]).await,
        Err(OnboardWriteError::ActiveProfileCorrupt(0x0001))
    );
    assert!(mouse.lock().unwrap().committed.is_empty());
}

#[tokio::test]
async fn a_slot_past_the_buttons_is_refused() {
    let mouse = Arc::new(Mutex::new(Mouse::g502x()));
    let feature = feature(&mouse).await;
    let past = OnboardBindingChange {
        slot: 11,
        ..g8_to_key_a()
    };

    assert_matches!(
        remap_on(&feature, &[past]).await,
        Err(OnboardWriteError::NoSuchButton {
            slot: 11,
            button_count: 11
        })
    );
    assert!(mouse.lock().unwrap().committed.is_empty());
}

#[tokio::test]
async fn a_write_the_flash_drops_fails_verification() {
    let mouse = Arc::new(Mutex::new(Mouse {
        commits: false,
        ..Mouse::g502x()
    }));
    let feature = feature(&mouse).await;

    assert_matches!(
        remap_on(&feature, &[g8_to_key_a()]).await,
        Err(OnboardWriteError::VerifyFailed(0x0001))
    );
}

#[tokio::test]
async fn a_backup_restores_the_original_sector() {
    let mouse = Arc::new(Mutex::new(Mouse::g502x()));
    let feature = feature(&mouse).await;
    let remap = remap_on(&feature, &[g8_to_key_a()]).await.unwrap();

    let restore = restore_on(&feature, remap.before.clone()).await.unwrap();

    assert!(restore.changed());
    assert_eq!(restore.before, remap.after);
    assert_eq!(mouse.lock().unwrap().sectors[&0x0001], stock_profile());
}

#[tokio::test]
async fn an_invalid_backup_is_refused_unwritten() {
    let mouse = Arc::new(Mutex::new(Mouse::g502x()));
    let feature = feature(&mouse).await;
    let mut tampered = stock_profile();
    tampered[0x30] ^= 1;

    for backup in [stock_profile()[..200].to_vec(), tampered] {
        assert_matches!(
            restore_on(&feature, backup).await,
            Err(OnboardWriteError::InvalidBackup { expected: 255 })
        );
    }
    assert!(mouse.lock().unwrap().committed.is_empty());
}
