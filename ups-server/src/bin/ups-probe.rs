//! Headless HID probe: enumerate APC UPS units, parse their report
//! descriptors, and dump what our parser extracts — to validate against real
//! firmware without the GUI/API in the way.
//!
//! Run: `cargo run -p ups-server --bin ups-probe`
//! Add `--smart <serial>` to also query APC's legacy "smart protocol" ASCII
//! tunnel (vendor HID reports 0x90/0x96) for telemetry the standard Power
//! Device usage page doesn't expose on some models (e.g. load %). Only
//! read-only, well-documented status query letters are sent — no shutdown or
//! control commands.

use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use hidapi::{HidApi, HidDevice};

#[path = "../hid.rs"]
mod hid;
#[path = "../hid_desc.rs"]
mod hid_desc;

/// Read one feature report (payload without the report-id byte). Duplicated
/// from `hid::read_feature` (private to that module) for probe diagnostics.
fn read_feature(dev: &HidDevice, report_id: u8) -> Option<Vec<u8>> {
    let mut buf = [0u8; 64];
    buf[0] = report_id;
    dev.get_feature_report(&mut buf)
        .ok()
        .map(|n| buf[1..n.min(buf.len())].to_vec())
}

fn usage_name(u: u32) -> String {
    let known = match u {
        0x8500D0 => "ACPresent",
        0x8500D6 => "Discharging",
        0x8500DB => "Charging",
        0x850066 => "RemainingCapacity",
        0x850068 => "RunTimeToEmpty",
        0x850085 => "BatteryPresent",
        0x840035 => "PercentLoad",
        0x840030 => "Voltage",
        _ => "",
    };
    if known.is_empty() {
        format!("0x{u:06x}")
    } else {
        format!("0x{u:06x} ({known})")
    }
}

/// APC legacy "smart protocol" output report id (host -> UPS command byte)
/// and the feature report id used to read the ASCII response back, tunneled
/// over the vendor usage page (0xff86) this model exposes alongside the
/// standard Power Device page.
const SMART_CMD_REPORT: u8 = 0x90;
const SMART_REPLY_REPORT: u8 = 0x96;

/// Send one single-letter, read-only smart-protocol status query and return
/// the ASCII reply. Only ever call this with documented, non-destructive
/// query letters (load %, voltages, frequency, temperature) — never a
/// control/shutdown command.
/// This UPS has no interrupt OUT endpoint, so `hidapi`'s generic `write()`
/// (which goes through `WriteFile`) fails with "Incorrect function". Output
/// reports on devices like this must go through the Windows HID
/// control-transfer API instead.
#[cfg(windows)]
fn set_output_report(path: &str, data: &[u8]) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Devices::HumanInterfaceDevice::HidD_SetOutputReport;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .map_err(|e| format!("CreateFileW failed: {e}"))?;

        let mut buf = data.to_vec();
        let result = HidD_SetOutputReport(
            windows::Win32::Foundation::HANDLE(handle.0),
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
        );
        let _ = CloseHandle(handle);
        if result {
            Ok(())
        } else {
            Err("HidD_SetOutputReport failed".to_string())
        }
    }
}

fn smart_query(dev: &HidDevice, path: &str, cmd: u8) -> Result<String, String> {
    let mut out = [0u8; 64];
    out[0] = SMART_CMD_REPORT;
    out[1] = cmd;
    #[cfg(windows)]
    set_output_report(path, &out)?;
    #[cfg(not(windows))]
    {
        let _ = path;
        dev.write(&out)
            .map_err(|e| format!("write(0x{SMART_CMD_REPORT:02x}) failed: {e}"))?;
    }
    // Poll for the response: the UPS can take a while to populate it, and an
    // all-null reply just means "not ready yet" rather than "no data".
    for attempt in 0..20 {
        sleep(Duration::from_millis(100));
        let mut reply = [0u8; 64];
        reply[0] = SMART_REPLY_REPORT;
        let n = dev
            .get_feature_report(&mut reply)
            .map_err(|e| format!("get_feature_report(0x{SMART_REPLY_REPORT:02x}) failed: {e}"))?;
        let text = String::from_utf8_lossy(&reply[1..n.min(reply.len())]);
        let trimmed = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
        if !trimmed.is_empty() {
            return Ok(format!("{trimmed} (after {} polls)", attempt + 1));
        }
    }
    Ok(String::new())
}

fn run_smart_probe(dev: &HidDevice, path: &str) {
    println!("  -- smart protocol queries --");
    // (letter, label) — all read-only telemetry queries.
    const QUERIES: &[(u8, &str)] = &[
        (b'P', "load %"),
        (b'L', "line (input) voltage"),
        (b'O', "output voltage"),
        (b'F', "line frequency"),
        (b'B', "battery voltage"),
        (b'C', "internal temperature"),
    ];
    for &(cmd, label) in QUERIES {
        match smart_query(dev, path, cmd) {
            Ok(reply) if !reply.is_empty() => {
                println!("    '{}' ({label}) -> {:?}", cmd as char, reply)
            }
            Ok(_) => println!("    '{}' ({label}) -> <empty reply>", cmd as char),
            Err(e) => println!("    '{}' ({label}) -> {e}", cmd as char),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let smart_serial = args
        .iter()
        .position(|a| a == "--smart")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let api = HidApi::new()?;
    let found = hid::enumerate_ups(&api);

    if found.is_empty() {
        println!("No APC UPS units found (vendor 0x051D, usage page 0x84).");
        return Ok(());
    }

    for d in &found {
        println!("=== {} ({}) ===", d.product, d.serial);
        let dev = match hid::open_by_serial(&api, &d.serial) {
            Some(dev) => dev,
            None => {
                println!("  !! could not open device");
                continue;
            }
        };

        let mut desc_buf = vec![0u8; hidapi::MAX_REPORT_DESCRIPTOR_SIZE];
        let n = dev.get_report_descriptor(&mut desc_buf)?;
        println!("  descriptor: {n} bytes");
        // Hex dump the descriptor for offline analysis.
        let hex: Vec<String> = desc_buf[..n].iter().map(|b| format!("{b:02x}")).collect();
        for chunk in hex.chunks(16) {
            println!("    {}", chunk.join(" "));
        }
        let layouts = hid_desc::parse_feature_layouts(&desc_buf[..n]);
        for l in &layouts {
            println!("  feature report id {}:", l.report_id);
            let payload = read_feature(&dev, l.report_id);
            for f in &l.fields {
                let val = payload
                    .as_ref()
                    .and_then(|p| hid_desc::extract(p, f));
                println!(
                    "    {} @ bit {}, {} bits  raw={:?}",
                    usage_name(f.usage),
                    f.bit_offset,
                    f.bit_size,
                    val
                );
            }
        }

        match hid::read_status(&dev, "probe") {
            Ok(s) => println!(
                "  parsed: status={} battery={:?}% runtime={:?}s load={:?}% voltage={:?}V",
                s.status, s.battery_pct, s.runtime_secs, s.load_pct, s.voltage_v
            ),
            Err(e) => println!("  !! read_status error: {e:#}"),
        }

        if smart_serial.as_deref() == Some(d.serial.as_str()) {
            run_smart_probe(&dev, &d.path);
        }
    }

    Ok(())
}
