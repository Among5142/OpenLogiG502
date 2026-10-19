//! `openlogi diag profiles` — dump a G-series mouse's `0x8100` onboard
//! profile store: what `getInfo` reports, the mode, the active sector, the
//! user directory, and every user and ROM profile with its decoded button
//! bindings and raw bytes.
//!
//! Read-only. It exists so the reverse-engineered sector layout can be
//! checked against a real device before anything writes to its flash.

use std::fmt::Write as _;

use anyhow::{Context, Result};
use clap::Args;
use openlogi_hid::onboard_profiles::layout::{BINDING_SLOTS, ButtonBinding, UnknownBinding};
use openlogi_hid::write::{OnboardProfilesDump, SectorCrc, SectorDump};

use crate::cmd::diag::select_device;

/// The `OnboardProfiles` feature id.
const ONBOARD_PROFILES: u16 = 0x8100;

#[derive(Debug, Args)]
pub struct ProfilesArgs {
    /// Run against the device whose name contains this string
    /// (case-insensitive) instead of auto-selecting.
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
    /// Skip the raw hex of each sector.
    #[arg(long)]
    pub no_hex: bool,
}

pub async fn run(args: ProfilesArgs) -> Result<()> {
    let (route, name) = select_device(args.device.as_deref(), &[ONBOARD_PROFILES]).await?;
    println!("device: {name} ({route})");
    let dump = openlogi_hid::dump_onboard_profiles(&route)
        .await
        .context("read 0x8100 onboard profiles")?;
    print!("{}", render(&dump, !args.no_hex));
    Ok(())
}

fn render(dump: &OnboardProfilesDump, hex: bool) -> String {
    let mut out = String::new();
    let info = &dump.info;
    let _ = writeln!(
        out,
        "  info: memory_model=0x{:02x} profile_format=0x{:02x} macro_format=0x{:02x}",
        info.memory_model, info.profile_format, info.macro_format
    );
    let _ = writeln!(
        out,
        "        profiles={} rom_profiles={} buttons={} sectors={} sector_size={}",
        info.profile_count,
        info.rom_profile_count,
        info.button_count,
        info.sector_count,
        info.sector_size
    );
    let _ = writeln!(
        out,
        "        mechanical_layout=0x{:02x} (g-shift={}, dpi-shift={}) various_info=0x{:02x}",
        info.mechanical_layout.0,
        info.mechanical_layout.has_gshift(),
        info.mechanical_layout.has_dpi_shift(),
        info.various_info
    );
    let _ = writeln!(
        out,
        "  mode: {:?}  current sector: 0x{:04x}",
        dump.mode, dump.current_sector
    );

    let Some(directory) = &dump.directory else {
        let _ = writeln!(out, "  layout not supported by this build; no sectors read");
        return out;
    };
    let _ = writeln!(out, "  directory (crc {}):", crc_label(directory.crc_valid));
    match &directory.entries {
        Ok(entries) if entries.is_empty() => {
            let _ = writeln!(out, "    (empty)");
        }
        Ok(entries) => {
            for entry in entries {
                let _ = writeln!(
                    out,
                    "    sector 0x{:04x} enabled={}",
                    entry.sector, entry.enabled
                );
            }
        }
        Err(e) => {
            let _ = writeln!(out, "    undecodable: {e}");
        }
    }

    let buttons = usize::from(info.button_count).min(BINDING_SLOTS);
    let gshift = info.mechanical_layout.has_gshift();
    for sector in &dump.sectors {
        render_sector(&mut out, sector, buttons, gshift, hex);
    }
    out
}

fn render_sector(out: &mut String, sector: &SectorDump, buttons: usize, gshift: bool, hex: bool) {
    let kind = if sector.rom { "rom" } else { "user" };
    let enabled = sector
        .enabled
        .map_or(String::new(), |e| format!(" enabled={e}"));
    let _ = writeln!(
        out,
        "  {kind} profile, sector 0x{:04x}{enabled} (crc {}):",
        sector.sector,
        match sector.crc {
            SectorCrc::Valid => crc_label(true),
            SectorCrc::Mismatch => crc_label(false),
            SectorCrc::NotStored => "not stored",
        }
    );
    match &sector.profile {
        Ok(profile) => {
            let _ = writeln!(
                out,
                "    name={:?} report_interval={}ms dpi={:?} default_index={} shift_index={}",
                profile.name.as_deref().unwrap_or("(default)"),
                profile.report_interval_ms,
                profile.dpi,
                profile.default_dpi_index,
                profile.shift_dpi_index
            );
            for (i, binding) in profile.buttons.iter().take(buttons).enumerate() {
                let _ = writeln!(out, "    button {:>2}: {}", i + 1, binding_label(*binding));
            }
            if gshift {
                for (i, binding) in profile.gshift_buttons.iter().take(buttons).enumerate() {
                    let _ = writeln!(out, "    g-shift {:>2}: {}", i + 1, binding_label(*binding));
                }
            }
        }
        Err(e) => {
            let _ = writeln!(out, "    undecodable: {e}");
        }
    }
    if hex {
        for (row, chunk) in sector.raw.chunks(16).enumerate() {
            let bytes: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let _ = writeln!(out, "    {:04x}: {}", row * 16, bytes.join(" "));
        }
    }
}

fn crc_label(valid: bool) -> &'static str {
    if valid { "ok" } else { "MISMATCH" }
}

pub(crate) fn binding_label(binding: Result<ButtonBinding, UnknownBinding>) -> String {
    match binding {
        Ok(ButtonBinding::Unassigned) => "unassigned".to_owned(),
        Ok(ButtonBinding::NoAction) => "no action".to_owned(),
        Ok(ButtonBinding::Mouse(buttons)) => format!("mouse {buttons:?}"),
        Ok(ButtonBinding::Key { modifiers, usage }) if modifiers.is_empty() => {
            format!("key usage 0x{usage:02x}")
        }
        Ok(ButtonBinding::Key { modifiers, usage }) => {
            format!("key usage 0x{usage:02x} + {modifiers:?}")
        }
        Ok(ButtonBinding::Consumer(usage)) => format!("consumer usage 0x{usage:04x}"),
        Ok(ButtonBinding::Function { function, data: 0 }) => format!("{function:?}"),
        Ok(ButtonBinding::Function { function, data }) => format!("{function:?} (data {data})"),
        Ok(ButtonBinding::Macro {
            behavior,
            sector,
            address,
        }) => format!("macro {behavior:?} at sector 0x{sector:03x} + 0x{address:04x}"),
        Err(unknown) => unknown.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use openlogi_hid::onboard_profiles::layout::{self, DirectoryEntry, ProfileSector};
    use openlogi_hid::onboard_profiles::{MechanicalLayout, OnboardMode, OnboardProfilesInfo};
    use openlogi_hid::write::{DirectoryDump, OnboardProfilesDump, SectorCrc, SectorDump};

    use super::render;

    fn info(profile_format: u8) -> OnboardProfilesInfo {
        OnboardProfilesInfo {
            memory_model: 1,
            profile_format,
            macro_format: 1,
            profile_count: 1,
            rom_profile_count: 1,
            button_count: 3,
            sector_count: 16,
            sector_size: 255,
            mechanical_layout: MechanicalLayout(0x0a),
            various_info: 0x02,
        }
    }

    fn sector() -> Vec<u8> {
        let mut raw = vec![0xff; 255];
        raw[0..3].copy_from_slice(&[1, 0, 1]);
        raw[0x20..0x2c].copy_from_slice(&[
            0x80, 0x01, 0x00, 0x01, //
            0x90, 0x07, 0xff, 0x00, //
            0x80, 0x09, 0x00, 0x00,
        ]);
        let crc = layout::crc_ccitt(&raw[..253]);
        raw[253..].copy_from_slice(&crc.to_be_bytes());
        raw
    }

    #[test]
    fn a_dump_renders_bindings_crc_and_hex() {
        let raw = sector();
        let dump = OnboardProfilesDump {
            info: info(5),
            mode: OnboardMode::Onboard,
            current_sector: 1,
            directory: Some(DirectoryDump {
                crc_valid: true,
                entries: Ok(vec![DirectoryEntry {
                    sector: 1,
                    enabled: true,
                }]),
            }),
            sectors: vec![SectorDump {
                sector: 1,
                rom: false,
                enabled: Some(true),
                crc: SectorCrc::Valid,
                profile: ProfileSector::decode(&raw),
                raw,
            }],
        };
        let text = render(&dump, true);
        assert!(text.contains("profile_format=0x05"), "{text}");
        assert!(text.contains("g-shift=true, dpi-shift=true"), "{text}");
        assert!(
            text.contains("sector 0x0001 enabled=true (crc ok)"),
            "{text}"
        );
        assert!(
            text.contains("button  1: mouse MouseButtons(LEFT)"),
            "{text}"
        );
        assert!(text.contains("button  2: ShiftDpi"), "{text}");
        assert!(
            text.contains("button  3: unrecognised button binding [80, 09, 00, 00]"),
            "{text}"
        );
        assert!(text.contains("g-shift  1: unassigned"), "{text}");
        assert!(!text.contains("button  4"), "{text}");
        assert!(text.contains("    00f0: "), "{text}");
    }

    #[test]
    fn an_unsupported_layout_stops_after_info() {
        let dump = OnboardProfilesDump {
            info: info(9),
            mode: OnboardMode::Host,
            current_sector: 0,
            directory: None,
            sectors: Vec::new(),
        };
        let text = render(&dump, true);
        assert!(text.contains("layout not supported"), "{text}");
        assert!(!text.contains("directory"), "{text}");
    }
}
