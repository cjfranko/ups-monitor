//! UPS enumeration and polling over USB HID (APC units, Power Device class).
//!
//! Rather than guessing APC report ids, we fetch the device's HID report
//! descriptor, parse the feature-report layouts (`hid_desc`), and look up the
//! standard Power Device usages we care about:
//!
//!   0x8500D0 ACPresent          -> mains OK
//!   0x8500D6 Discharging        -> running on battery
//!   0x8500DB Charging           -> battery charging
//!   0x850066 RemainingCapacity  -> battery %
//!   0x850068 RunTimeToEmpty     -> runtime seconds (while discharging)
//!
//! Everything that touches hardware is isolated behind `read_status`; the
//! parsing of raw descriptors and report payloads is pure and unit-tested.
//!
//! If this proves unreliable on a given unit's firmware, the documented escape
//! hatch is to shell out to NUT's `upsc` — see the build plan. Not built yet.

use anyhow::{Context, Result};
use chrono::Utc;
use hidapi::{DeviceInfo, HidApi, HidDevice};
use ups_common::{PowerState, UpsStatus};

use crate::hid_desc::{self, FieldLoc, ReportLayout};

/// USB HID Power Device usage page.
pub const UPS_USAGE_PAGE: u16 = 0x84;
/// Battery System usage page.
pub const BATTERY_USAGE_PAGE: u16 = 0x85;
/// APC vendor id.
pub const APC_VENDOR_ID: u16 = 0x051D;

const USAGE_AC_PRESENT: u32 = (BATTERY_USAGE_PAGE as u32) << 16 | 0xD0;
const USAGE_DISCHARGING: u32 = (BATTERY_USAGE_PAGE as u32) << 16 | 0xD6;
const USAGE_REMAINING_CAPACITY: u32 = (BATTERY_USAGE_PAGE as u32) << 16 | 0x66;
const USAGE_RUNTIME_TO_EMPTY: u32 = (BATTERY_USAGE_PAGE as u32) << 16 | 0x68;

/// A discovered UPS device (not yet opened).
#[derive(Debug, Clone)]
pub struct DiscoveredUps {
    /// OS device path (used by the probe tool for diagnostics).
    #[allow(dead_code)]
    pub path: String,
    pub serial: String,
    pub product: String,
}

/// Enumerate connected APC UPS units (usage page 0x84, vendor 0x051D).
pub fn enumerate_ups(api: &HidApi) -> Vec<DiscoveredUps> {
    api.device_list()
        .filter(|d: &&DeviceInfo| {
            d.vendor_id() == APC_VENDOR_ID && d.usage_page() == UPS_USAGE_PAGE
        })
        .map(|d| DiscoveredUps {
            path: d.path().to_string_lossy().to_string(),
            // APC pads serials with trailing spaces; trim for config matching.
            serial: d.serial_number().unwrap_or_default().trim().to_string(),
            product: d.product_string().unwrap_or_default().to_string(),
        })
        .collect()
}

/// Open a UPS by serial number. Returns `None` if not found right now.
/// Comparison trims whitespace on both sides (APC pads serials with spaces).
pub fn open_by_serial(api: &HidApi, serial: &str) -> Option<HidDevice> {
    let wanted = serial.trim();
    api.device_list()
        .find(|d| {
            d.vendor_id() == APC_VENDOR_ID
                && d.usage_page() == UPS_USAGE_PAGE
                && d.serial_number().unwrap_or_default().trim() == wanted
        })
        .and_then(|d| d.open_device(api).ok())
}

/// Which usages we found and where, per report id.
#[derive(Debug, Clone, Default)]
struct Located {
    ac_present: Option<(u8, FieldLoc)>,
    discharging: Option<(u8, FieldLoc)>,
    remaining_capacity: Option<(u8, FieldLoc)>,
    runtime_to_empty: Option<(u8, FieldLoc)>,
}

fn locate(layouts: &[ReportLayout]) -> Located {
    let mut out = Located::default();
    for l in layouts {
        for f in &l.fields {
            let slot = match f.usage {
                u if u == USAGE_AC_PRESENT => &mut out.ac_present,
                u if u == USAGE_DISCHARGING => &mut out.discharging,
                u if u == USAGE_REMAINING_CAPACITY => &mut out.remaining_capacity,
                u if u == USAGE_RUNTIME_TO_EMPTY => &mut out.runtime_to_empty,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some((l.report_id, *f));
            }
        }
    }
    out
}

/// Read one feature report (payload without the report-id byte).
fn read_feature(dev: &HidDevice, report_id: u8) -> Option<Vec<u8>> {
    let mut buf = [0u8; 64];
    buf[0] = report_id;
    dev.get_feature_report(&mut buf)
        .ok()
        .map(|n| buf[1..n.min(buf.len())].to_vec())
}

/// Read one status snapshot from an open device.
pub fn read_status(dev: &HidDevice, id: &str) -> Result<UpsStatus> {
    let mut desc_buf = vec![0u8; hidapi::MAX_REPORT_DESCRIPTOR_SIZE];
    let n = dev
        .get_report_descriptor(&mut desc_buf)
        .context("failed to fetch HID report descriptor")?;
    let layouts = hid_desc::parse_feature_layouts(&desc_buf[..n]);
    let located = locate(&layouts);

    // Power state from ACPresent / Discharging bits (they may live in
    // different reports).
    let ac_present = located
        .ac_present
        .and_then(|(rid, loc)| read_feature(dev, rid).and_then(|p| hid_desc::extract(&p, &loc)))
        .map(|v| v != 0);
    let discharging = located
        .discharging
        .and_then(|(rid, loc)| read_feature(dev, rid).and_then(|p| hid_desc::extract(&p, &loc)))
        .map(|v| v != 0);

    let status = derive_status(ac_present, discharging);

    let battery_pct = located
        .remaining_capacity
        .and_then(|(rid, loc)| read_feature(dev, rid).and_then(|p| hid_desc::extract(&p, &loc)))
        .map(|v| v.min(100) as u8);

    let runtime_secs = located
        .runtime_to_empty
        .and_then(|(rid, loc)| read_feature(dev, rid).and_then(|p| hid_desc::extract(&p, &loc)))
        .map(|v| v as u32);

    Ok(UpsStatus {
        id: id.to_string(),
        status,
        battery_pct,
        runtime_secs,
        last_updated: Utc::now(),
    })
}

/// Pure, testable: derive `PowerState` from ACPresent / Discharging flags.
pub fn derive_status(ac_present: Option<bool>, discharging: Option<bool>) -> PowerState {
    match (ac_present, discharging) {
        (_, Some(true)) => PowerState::OnBattery,
        (Some(false), _) => PowerState::OnBattery,
        (Some(true), _) => PowerState::Online,
        (None, Some(false)) => PowerState::Online,
        (None, None) => PowerState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_when_ac_present_and_not_discharging() {
        assert_eq!(derive_status(Some(true), Some(false)), PowerState::Online);
        assert_eq!(derive_status(Some(true), None), PowerState::Online);
    }

    #[test]
    fn on_battery_when_discharging() {
        assert_eq!(derive_status(Some(true), Some(true)), PowerState::OnBattery);
        assert_eq!(derive_status(None, Some(true)), PowerState::OnBattery);
    }

    #[test]
    fn on_battery_when_ac_absent() {
        assert_eq!(derive_status(Some(false), Some(false)), PowerState::OnBattery);
        assert_eq!(derive_status(Some(false), None), PowerState::OnBattery);
    }

    #[test]
    fn unknown_when_no_data() {
        assert_eq!(derive_status(None, None), PowerState::Unknown);
    }

    #[test]
    fn parses_minimal_apc_style_descriptor() {
        // Hand-built descriptor:
        //   Usage Page (Power Device 0x84)
        //   Usage (UPS 0x04), Collection (Application)
        //     Report ID 1
        //     Usage Page (Battery System 0x85)
        //     Usage (ACPresent 0xD0), Usage (Discharging 0xD6)
        //     Logical Min 0, Max 1, Report Size 1, Count 2
        //     Feature (Data,Var,Abs)
        //     Usage (RemainingCapacity 0x66)
        //     Logical Max 100, Report Size 8, Count 1
        //     Feature (Data,Var,Abs)
        //   End Collection
        let desc: &[u8] = &[
            0x05, 0x84, // Usage Page (Power Device)
            0x09, 0x04, // Usage (UPS)
            0xA1, 0x01, // Collection (Application)
            0x85, 0x01, //   Report ID (1)
            0x05, 0x85, //   Usage Page (Battery System)
            0x09, 0xD0, //   Usage (ACPresent)
            0x09, 0xD6, //   Usage (Discharging)
            0x15, 0x00, //   Logical Minimum (0)
            0x25, 0x01, //   Logical Maximum (1)
            0x75, 0x01, //   Report Size (1)
            0x95, 0x02, //   Report Count (2)
            0xB1, 0x02, //   Feature (Data,Var,Abs)
            0x09, 0x66, //   Usage (RemainingCapacity)
            0x25, 0x64, //   Logical Maximum (100)
            0x75, 0x08, //   Report Size (8)
            0x95, 0x01, //   Report Count (1)
            0xB1, 0x02, //   Feature (Data,Var,Abs)
            0xC0, // End Collection
        ];
        let layouts = hid_desc::parse_feature_layouts(desc);
        assert_eq!(layouts.len(), 1);
        let l = &layouts[0];
        assert_eq!(l.report_id, 1);
        // ACPresent at bit 0, Discharging at bit 1, RemainingCapacity at bit 2.
        assert_eq!(l.fields[0].usage, USAGE_AC_PRESENT);
        assert_eq!(l.fields[0].bit_offset, 0);
        assert_eq!(l.fields[0].bit_size, 1);
        assert_eq!(l.fields[1].usage, USAGE_DISCHARGING);
        assert_eq!(l.fields[1].bit_offset, 1);
        assert_eq!(l.fields[2].usage, USAGE_REMAINING_CAPACITY);
        assert_eq!(l.fields[2].bit_offset, 2);
        assert_eq!(l.fields[2].bit_size, 8);

        // Payload: bits 0..1 = 0b10 (discharging), capacity 85 at bit 2.
        // 85 << 2 = 0b101010100 -> bytes [0b0101_0100, 0b0000_0101].
        let payload = [0b0101_0110u8, 0b0000_0101];
        assert_eq!(hid_desc::extract(&payload, &l.fields[0]), Some(0));
        assert_eq!(hid_desc::extract(&payload, &l.fields[1]), Some(1));
        assert_eq!(hid_desc::extract(&payload, &l.fields[2]), Some(85));
    }
}
