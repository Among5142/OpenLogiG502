//! `openlogi diag remap` / `openlogi diag restore` — change one binding in a
//! G-series mouse's active `0x8100` onboard profile, and put a backup back.
//!
//! These write device flash. `remap` first saves the active profile sector
//! to a backup file in the current directory and refuses to continue if it
//! cannot; `restore` writes such a file back. The device layer refuses
//! factory profiles, an invalid directory, a corrupt active sector and slots
//! past the mouse's buttons, and verifies every write by reading it back.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use openlogi_hid::onboard_profiles::layout::{
    BindingLayer, ButtonBinding, ButtonFunction, KeyModifiers, MouseButtons, ProfileSector,
};
use openlogi_hid::write::OnboardBindingChange;

use crate::cmd::diag::profiles::binding_label;
use crate::cmd::diag::select_device;

/// The `OnboardProfiles` feature id.
const ONBOARD_PROFILES: u16 = 0x8100;

#[derive(Debug, Args)]
pub struct RemapArgs {
    /// Button slot, 1-based, as `diag profiles` numbers them. On a G502 X:
    /// 1 G1 left, 2 G2 right, 3 G3 middle, 4 G4 back, 5 G6 DPI shift,
    /// 6 G5 forward, 7/8 wheel tilt left/right, 9 G9 profile, 10 G8 DPI up,
    /// 11 G7 DPI down.
    #[arg(long)]
    pub slot: usize,
    /// Change the G-shift layer instead of the normal bindings.
    #[arg(long)]
    pub gshift: bool,
    /// The new binding: `mouse:left|right|middle|back|forward`,
    /// `key:0x04` (HID usage) or `key:0x04:ctrl+shift`, `consumer:0xe9`,
    /// `function:next-dpi` (also previous-dpi, cycle-dpi, default-dpi,
    /// shift-dpi, next-profile, previous-profile, cycle-profile, g-shift,
    /// tilt-left, tilt-right, scroll-up, scroll-down, battery-status,
    /// mode-switch, host-button, no-action), `none`, or `unassigned`.
    #[arg(long, value_name = "BINDING")]
    pub to: String,
    /// Run against the device whose name contains this string
    /// (case-insensitive) instead of auto-selecting.
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
}

#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// A backup file written by `diag remap`.
    pub backup: PathBuf,
    /// Run against the device whose name contains this string
    /// (case-insensitive) instead of auto-selecting.
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
}

pub async fn run_remap(args: RemapArgs) -> Result<()> {
    let slot = args
        .slot
        .checked_sub(1)
        .ok_or_else(|| anyhow!("--slot counts from 1"))?;
    let binding = parse_binding(&args.to)?;
    let layer = if args.gshift {
        BindingLayer::GShift
    } else {
        BindingLayer::Primary
    };

    let (route, name) = select_device(args.device.as_deref(), &[ONBOARD_PROFILES]).await?;
    println!("device: {name} ({route})");

    // Back the active profile up before anything is written.
    let dump = openlogi_hid::dump_onboard_profiles(&route)
        .await
        .context("read 0x8100 onboard profiles")?;
    let active = dump
        .sectors
        .iter()
        .find(|sector| !sector.rom && sector.sector == dump.current_sector)
        .ok_or_else(|| {
            anyhow!(
                "the active profile (sector 0x{:04x}) is not a writable user profile",
                dump.current_sector
            )
        })?;
    let backup = backup_path(active.sector);
    std::fs::write(&backup, &active.raw)
        .with_context(|| format!("save backup to {}; nothing was written", backup.display()))?;
    println!("  backup: {}", backup.display());

    let change = OnboardBindingChange {
        layer,
        slot,
        binding,
    };
    let outcome = openlogi_hid::remap_onboard_buttons(&route, vec![change])
        .await
        .context("remap")?;
    if outcome.before != active.raw {
        println!("  note: the profile changed between the backup and the write");
    }
    let label = |raw: &[u8]| {
        ProfileSector::decode(raw).map_or_else(
            |e| e.to_string(),
            |profile| {
                let table = match layer {
                    BindingLayer::Primary => profile.buttons,
                    BindingLayer::GShift => profile.gshift_buttons,
                };
                binding_label(table[slot])
            },
        )
    };
    let layer_name = if args.gshift { "g-shift" } else { "button" };
    if outcome.changed() {
        println!(
            "  sector 0x{:04x} {layer_name} {}: {} -> {} (written and verified)",
            outcome.sector,
            args.slot,
            label(&outcome.before),
            label(&outcome.after)
        );
    } else {
        println!(
            "  sector 0x{:04x} {layer_name} {} is already {}; nothing written",
            outcome.sector,
            args.slot,
            label(&outcome.after)
        );
    }
    Ok(())
}

pub async fn run_restore(args: RestoreArgs) -> Result<()> {
    let backup =
        std::fs::read(&args.backup).with_context(|| format!("read {}", args.backup.display()))?;
    let (route, name) = select_device(args.device.as_deref(), &[ONBOARD_PROFILES]).await?;
    println!("device: {name} ({route})");
    let outcome = openlogi_hid::restore_onboard_profile(&route, backup)
        .await
        .context("restore")?;
    if outcome.changed() {
        println!(
            "  sector 0x{:04x} restored from {} (written and verified)",
            outcome.sector,
            args.backup.display()
        );
    } else {
        println!(
            "  sector 0x{:04x} already matches {}; nothing written",
            outcome.sector,
            args.backup.display()
        );
    }
    Ok(())
}

/// `onboard-profile-0001-<unix seconds>.bin` in the current directory.
fn backup_path(sector: u16) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    PathBuf::from(format!("onboard-profile-{sector:04x}-{stamp}.bin"))
}

/// Parses a `--to` binding spec; see [`RemapArgs::to`].
fn parse_binding(spec: &str) -> Result<ButtonBinding> {
    let spec = spec.trim().to_ascii_lowercase();
    let mut parts = spec.split(':');
    let kind = parts.next().unwrap_or_default();
    let value = parts.next();
    let extra = parts.next();
    if parts.next().is_some() {
        bail!("too many `:` in binding {spec:?}");
    }
    let binding = match (kind, value, extra) {
        ("unassigned", None, None) => ButtonBinding::Unassigned,
        ("none", None, None) => ButtonBinding::NoAction,
        ("mouse", Some(button), None) => ButtonBinding::Mouse(match button {
            "left" => MouseButtons::LEFT,
            "right" => MouseButtons::RIGHT,
            "middle" => MouseButtons::MIDDLE,
            "back" => MouseButtons::BACK,
            "forward" => MouseButtons::FORWARD,
            other => bail!("unknown mouse button {other:?}"),
        }),
        ("key", Some(usage), modifiers) => ButtonBinding::Key {
            usage: parse_number(usage)
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| anyhow!("key usage {usage:?} is not a byte"))?,
            modifiers: modifiers.map_or(Ok(KeyModifiers::empty()), parse_modifiers)?,
        },
        ("consumer", Some(usage), None) => ButtonBinding::Consumer(
            parse_number(usage)
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| anyhow!("consumer usage {usage:?} is not 16-bit"))?,
        ),
        ("function", Some(name), None) => ButtonBinding::Function {
            function: parse_function(name)?,
            data: 0,
        },
        _ => bail!("unrecognised binding {spec:?}; see `diag remap --help`"),
    };
    Ok(binding)
}

fn parse_number(text: &str) -> Option<u32> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

fn parse_modifiers(text: &str) -> Result<KeyModifiers> {
    text.split('+')
        .try_fold(KeyModifiers::empty(), |mods, name| {
            let flag = match name {
                "ctrl" => KeyModifiers::LEFT_CTRL,
                "shift" => KeyModifiers::LEFT_SHIFT,
                "alt" => KeyModifiers::LEFT_ALT,
                "gui" | "win" | "cmd" => KeyModifiers::LEFT_GUI,
                "rctrl" => KeyModifiers::RIGHT_CTRL,
                "rshift" => KeyModifiers::RIGHT_SHIFT,
                "ralt" => KeyModifiers::RIGHT_ALT,
                "rgui" => KeyModifiers::RIGHT_GUI,
                other => bail!("unknown modifier {other:?}"),
            };
            Ok(mods | flag)
        })
}

fn parse_function(name: &str) -> Result<ButtonFunction> {
    Ok(match name {
        "no-action" => ButtonFunction::NoAction,
        "tilt-left" => ButtonFunction::TiltLeft,
        "tilt-right" => ButtonFunction::TiltRight,
        "next-dpi" => ButtonFunction::NextDpi,
        "previous-dpi" => ButtonFunction::PreviousDpi,
        "cycle-dpi" => ButtonFunction::CycleDpi,
        "default-dpi" => ButtonFunction::DefaultDpi,
        "shift-dpi" => ButtonFunction::ShiftDpi,
        "next-profile" => ButtonFunction::NextProfile,
        "previous-profile" => ButtonFunction::PreviousProfile,
        "cycle-profile" => ButtonFunction::CycleProfile,
        "g-shift" => ButtonFunction::GShift,
        "battery-status" => ButtonFunction::BatteryStatus,
        "mode-switch" => ButtonFunction::ModeSwitch,
        "host-button" => ButtonFunction::HostButton,
        "scroll-down" => ButtonFunction::ScrollDown,
        "scroll-up" => ButtonFunction::ScrollUp,
        other => bail!("unknown function {other:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_specs_parse() {
        assert_eq!(
            parse_binding("mouse:back").unwrap(),
            ButtonBinding::Mouse(MouseButtons::BACK)
        );
        assert_eq!(
            parse_binding("key:0x04").unwrap(),
            ButtonBinding::Key {
                modifiers: KeyModifiers::empty(),
                usage: 0x04,
            }
        );
        assert_eq!(
            parse_binding("KEY:0x2b:Ctrl+Shift").unwrap(),
            ButtonBinding::Key {
                modifiers: KeyModifiers::LEFT_CTRL | KeyModifiers::LEFT_SHIFT,
                usage: 0x2b,
            }
        );
        assert_eq!(
            parse_binding("consumer:0xe9").unwrap(),
            ButtonBinding::Consumer(0xe9)
        );
        assert_eq!(
            parse_binding("function:next-dpi").unwrap(),
            ButtonBinding::Function {
                function: ButtonFunction::NextDpi,
                data: 0,
            }
        );
        assert_eq!(parse_binding("none").unwrap(), ButtonBinding::NoAction);
        assert_eq!(
            parse_binding("unassigned").unwrap(),
            ButtonBinding::Unassigned
        );
    }

    #[test]
    fn malformed_specs_are_errors() {
        for spec in [
            "mouse:side",
            "key:0x1ff",
            "key:0x04:hyper",
            "consumer:0x10000",
            "function:warp",
            "macro:0",
            "key:1:ctrl:extra",
            "",
        ] {
            assert!(parse_binding(spec).is_err(), "{spec:?} parsed");
        }
    }
}
