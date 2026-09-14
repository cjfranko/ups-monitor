//! Descriptor for one report: map of usage -> bit offset and size.

use std::collections::HashMap;

/// A single variable field inside a report.
#[derive(Debug, Clone, Copy)]
pub struct FieldLoc {
    /// Bit offset of this field within the report payload (after report id).
    pub bit_offset: usize,
    pub bit_size: usize,
    /// Full usage id: (usage_page << 16) | usage.
    pub usage: u32,
}

/// All fields of one feature report, keyed by full usage id.
#[derive(Debug, Clone, Default)]
pub struct ReportLayout {
    pub report_id: u8,
    pub fields: Vec<FieldLoc>,
}

/// Parse a raw HID report descriptor and return feature-report layouts:
/// for each report id, the bit position of every usage.
///
/// This is a minimal HID main-item parser — enough for the Power Device
/// descriptors APC ships. Global state (usage page, report id/size/count) is
/// tracked; local usages are attached to each main item; Delimiter/Physical
/// min/max and Push/Pop are not supported (APC doesn't need them).
pub fn parse_feature_layouts(desc: &[u8]) -> Vec<ReportLayout> {
    let mut layouts: HashMap<u8, Vec<FieldLoc>> = HashMap::new();
    let mut report_order: Vec<u8> = Vec::new();

    // Global state.
    let mut usage_page: u32 = 0;
    let mut report_size: usize = 0;
    let mut report_count: usize = 0;
    let mut report_id: u8 = 0;
    let mut _logical_min: i64 = 0;
    let mut _logical_max: i64 = 0;

    // Local state (reset after each main item).
    let mut usages: Vec<u32> = Vec::new();
    let mut usage_min: Option<u32> = None;
    let mut usage_max: Option<u32> = None;

    // Per-report bit cursor: (report_id, is_feature) -> next bit offset.
    let mut cursors: HashMap<u8, usize> = HashMap::new();

    let mut i = 0;
    while i < desc.len() {
        let b = desc[i];
        i += 1;
        if b == 0xFE {
            // Long item: skip.
            if i + 1 >= desc.len() {
                break;
            }
            let len = desc[i] as usize;
            i += 2 + len;
            continue;
        }
        let size = match b & 0x03 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        // HID short item: type is bits 2-3 (2 bits), tag is bits 4-7 (4 bits).
        let typ = (b >> 2) & 0x03;
        let tag = (b >> 4) & 0x0F;
        if i + size > desc.len() {
            break;
        }
        let mut val: u64 = 0;
        for k in 0..size {
            val |= (desc[i + k] as u64) << (8 * k);
        }
        let sval = sign_extend(val, size * 8);
        i += size;

        match (typ, tag) {
            // Global items (tags per HID spec 6.2.2.7).
            (0b01, 0x0) => usage_page = val as u32,
            (0b01, 0x1) => _logical_min = sval,
            (0b01, 0x2) => _logical_max = sval,
            (0b01, 0x7) => report_size = val as usize,
            (0b01, 0x8) => {
                report_id = val as u8;
                if !report_order.contains(&report_id) {
                    report_order.push(report_id);
                }
            }
            (0b01, 0x9) => report_count = val as usize,
            // Local items (tags per HID spec 6.2.2.8).
            (0b10, 0x0) => usages.push((usage_page << 16) | val as u32),
            (0b10, 0x1) => usage_min = Some(val as u32),
            (0b10, 0x2) => usage_max = Some(val as u32),
            // Main items: Input=0x8, Output=0x9, Feature=0xB.
            (0b00, 0x8) | (0b00, 0x9) | (0b00, 0xB) => {
                let is_feature = tag == 0xB;
                // Note: APC firmware marks many live status bits as
                // "Constant|Volatile|BufferedBytes" (0xa3) even though they
                // carry real data, so we do NOT filter on the constant bit.
                let is_variable = val & 0x02 != 0;

                let expanded: Vec<Option<u32>> = if is_variable {
                    expand_usages(usage_page, &usages, usage_min, usage_max, report_count)
                } else {
                    // Array main item: usages repeat over report_count slots;
                    // we treat them as opaque padding fields.
                    vec![None; report_count]
                };

                if is_feature {
                    let cursor = cursors.entry(report_id).or_insert(0);
                    let fields = layouts.entry(report_id).or_default();
                    for u in expanded.iter() {
                        if let Some(usage) = u {
                            fields.push(FieldLoc {
                                bit_offset: *cursor,
                                bit_size: report_size,
                                usage: *usage,
                            });
                        }
                        *cursor += report_size;
                    }
                } else {
                    // Input/Output still advance their own cursors if we ever
                    // need them; we only track feature reports.
                }

                usages.clear();
                usage_min = None;
                usage_max = None;
            }
            // Any other main item (Collection 0xA, End Collection 0xC) also
            // consumes/resets the local state.
            (0b00, _) => {
                usages.clear();
                usage_min = None;
                usage_max = None;
            }
            _ => {}
        }
    }

    report_order
        .into_iter()
        .filter_map(|id| {
            layouts.remove(&id).map(|fields| ReportLayout {
                report_id: id,
                fields,
            })
        })
        .collect()
}

fn expand_usages(
    usage_page: u32,
    usages: &[u32],
    usage_min: Option<u32>,
    usage_max: Option<u32>,
    count: usize,
) -> Vec<Option<u32>> {
    let mut out: Vec<Option<u32>> = Vec::with_capacity(count);
    if let (Some(min), Some(max)) = (usage_min, usage_max) {
        for u in min..=max {
            out.push(Some((usage_page << 16) | u));
        }
    }
    out.extend(usages.iter().copied().map(Some));
    out.resize(count, None);
    out.truncate(count);
    out
}

fn sign_extend(val: u64, bits: usize) -> i64 {
    if bits == 0 || bits >= 64 {
        return val as i64;
    }
    let shift = 64 - bits;
    ((val << shift) as i64) >> shift
}

/// Extract an unsigned field from a report payload (without report id byte).
pub fn extract(payload: &[u8], loc: &FieldLoc) -> Option<u64> {
    let bit = loc.bit_offset;
    let byte = bit / 8;
    let shift = bit % 8;
    let bytes_needed = (shift + loc.bit_size + 7) / 8;
    if byte + bytes_needed > payload.len() {
        return None;
    }
    let mut raw: u64 = 0;
    for k in 0..bytes_needed {
        raw |= (payload[byte + k] as u64) << (8 * k);
    }
    let mask = if loc.bit_size >= 64 {
        u64::MAX
    } else {
        (1u64 << loc.bit_size) - 1
    };
    Some((raw >> shift) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_extend_negative() {
        assert_eq!(sign_extend(0xFF, 8), -1);
        assert_eq!(sign_extend(0x7F, 8), 127);
        assert_eq!(sign_extend(0xFFFF, 16), -1);
    }

    #[test]
    fn extract_bits() {
        // 8-bit field at offset 8 of payload [0xAA, 0x05] -> 5.
        let loc = FieldLoc {
            bit_offset: 8,
            bit_size: 8,
            usage: 0,
        };
        assert_eq!(extract(&[0xAA, 0x05], &loc), Some(5));
        // 1-bit field at bit 1 of [0b10] -> 1.
        let loc1 = FieldLoc {
            bit_offset: 1,
            bit_size: 1,
            usage: 0,
        };
        assert_eq!(extract(&[0b10], &loc1), Some(1));
    }
}
