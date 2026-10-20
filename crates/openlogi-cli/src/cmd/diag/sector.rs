//! `openlogi diag sector` — dump raw sectors of a G-series mouse's `0x8100`
//! onboard profile store, such as the ones G HUB keeps macros in.
//!
//! Read-only. It exists so sector layouts this build does not decode yet
//! can be checked against a real device before anything writes them.

use std::fmt::Write as _;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use openlogi_hid::onboard_profiles::layout;

use crate::cmd::diag::select_device;

/// The `OnboardProfiles` feature id.
const ONBOARD_PROFILES: u16 = 0x8100;

#[derive(Debug, Args)]
pub struct SectorArgs {
    /// Sectors to dump, decimal or `0x` hex (e.g. `6` or `0x0101`).
    #[arg(required = true, value_parser = parse_sector)]
    pub sectors: Vec<u16>,
    /// Run against the device whose name contains this string
    /// (case-insensitive) instead of auto-selecting.
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
}

pub async fn run(args: SectorArgs) -> Result<()> {
    let (route, name) = select_device(args.device.as_deref(), &[ONBOARD_PROFILES]).await?;
    println!("device: {name} ({route})");
    for sector in args.sectors {
        let raw = openlogi_hid::read_onboard_sector(&route, sector)
            .await
            .with_context(|| format!("read sector 0x{sector:04x}"))?;
        print!("{}", render(sector, &raw));
    }
    Ok(())
}

fn parse_sector(text: &str) -> Result<u16> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|_| anyhow!("{text:?} is not a sector number"))
}

fn render(sector: u16, raw: &[u8]) -> String {
    let mut out = String::new();
    let crc = if layout::sector_crc_valid(raw) {
        "ok"
    } else {
        "not matching"
    };
    let _ = writeln!(
        out,
        "  sector 0x{sector:04x}, {} bytes (crc {crc}):",
        raw.len()
    );
    for (row, chunk) in raw.chunks(16).enumerate() {
        let bytes: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(out, "    {:04x}: {}", row * 16, bytes.join(" "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{parse_sector, render};

    #[test]
    fn sectors_parse_as_decimal_or_hex() {
        assert_eq!(parse_sector("6").unwrap(), 6);
        assert_eq!(parse_sector("0x0101").unwrap(), 0x0101);
        assert_eq!(parse_sector("0X6").unwrap(), 6);
        for invalid in ["", "six", "0x", "70000", "-1"] {
            assert!(parse_sector(invalid).is_err(), "{invalid:?} parsed");
        }
    }

    #[test]
    fn a_dump_shows_size_crc_and_rows() {
        let text = render(6, &[0xff; 20]);
        assert!(
            text.contains("sector 0x0006, 20 bytes (crc not matching)"),
            "{text}"
        );
        assert!(text.contains("    0010: ff ff ff ff"), "{text}");
    }
}
