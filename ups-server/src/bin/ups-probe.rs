//! Headless HID probe: enumerate APC UPS units, parse their report
//! descriptors, and dump what our parser extracts — to validate against real
//! firmware without the GUI/API in the way.
//!
//! Run: `cargo run -p ups-server --bin ups-probe`

use anyhow::Result;
use hidapi::HidApi;

#[path = "../hid.rs"]
mod hid;
#[path = "../hid_desc.rs"]
mod hid_desc;

fn usage_name(u: u32) -> String {
    let known = match u {
        0x8500D0 => "ACPresent",
        0x8500D6 => "Discharging",
        0x8500DB => "Charging",
        0x850066 => "RemainingCapacity",
        0x850068 => "RunTimeToEmpty",
        0x850085 => "BatteryPresent",
        _ => "",
    };
    if known.is_empty() {
        format!("0x{u:06x}")
    } else {
        format!("0x{u:06x} ({known})")
    }
}

fn main() -> Result<()> {
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
            for f in &l.fields {
                println!(
                    "    {} @ bit {}, {} bits",
                    usage_name(f.usage),
                    f.bit_offset,
                    f.bit_size
                );
            }
        }

        match hid::read_status(&dev, "probe") {
            Ok(s) => println!(
                "  parsed: status={} battery={:?}% runtime={:?}s",
                s.status, s.battery_pct, s.runtime_secs
            ),
            Err(e) => println!("  !! read_status error: {e:#}"),
        }
    }

    Ok(())
}
