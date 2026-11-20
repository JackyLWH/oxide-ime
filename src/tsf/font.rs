//! 讀取 Windows 所選字型的 Unicode cmap，支援補充平面的字元。

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn format4(table: &[u8], code: u32) -> Option<bool> {
    let code = u16::try_from(code).ok()?;
    let table = table.get(..usize::from(u16_at(table, 2)?))?;
    let segment_bytes = usize::from(u16_at(table, 6)?);
    if segment_bytes == 0 || segment_bytes % 2 != 0 {
        return None;
    }
    let end = 14;
    let start = end + segment_bytes + 2;
    let delta = start + segment_bytes;
    let ranges = delta + segment_bytes;
    let glyphs = ranges + segment_bytes;
    table.get(..glyphs)?;

    // Format 4 的 endCode 依升序排列。
    let (mut low, mut high) = (0, segment_bytes / 2);
    while low < high {
        let middle = low + (high - low) / 2;
        if u16_at(table, end + middle * 2)? < code {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let offset = low * 2;
    if offset == segment_bytes {
        return Some(false);
    }
    let first = u16_at(table, start + offset)?;
    if code < first {
        return Some(false);
    }
    let adjustment = u16_at(table, delta + offset)?;
    let range_offset = usize::from(u16_at(table, ranges + offset)?);
    let glyph = if range_offset == 0 {
        code.wrapping_add(adjustment)
    } else {
        let address = ranges + offset + range_offset + usize::from(code - first) * 2;
        if address < glyphs {
            return None;
        }
        let value = u16_at(table, address)?;
        if value == 0 {
            0
        } else {
            value.wrapping_add(adjustment)
        }
    };
    Some(glyph != 0)
}

fn format12(table: &[u8], code: u32) -> Option<bool> {
    let count = usize::try_from(u32_at(table, 12)?).ok()?;
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        let offset = 16usize.checked_add(middle.checked_mul(12)?)?;
        let first = u32_at(table, offset)?;
        let last = u32_at(table, offset + 4)?;
        if code < first {
            high = middle;
        } else if code > last {
            low = middle + 1;
        } else {
            return Some(u32_at(table, offset + 8)?.checked_add(code - first)? != 0);
        }
    }
    Some(false)
}

pub(crate) fn supports(cmap: &[u8], ch: char) -> bool {
    let Some(count) = u16_at(cmap, 2) else {
        return false;
    };
    for index in 0..usize::from(count) {
        let record = 4 + index * 8;
        let Some(platform) = u16_at(cmap, record) else {
            continue;
        };
        let Some(encoding) = u16_at(cmap, record + 2) else {
            continue;
        };
        if platform != 0 && !(platform == 3 && matches!(encoding, 1 | 10)) {
            continue;
        }
        let Some(offset) = u32_at(cmap, record + 4).and_then(|n| usize::try_from(n).ok()) else {
            continue;
        };
        let Some(table) = cmap.get(offset..) else {
            continue;
        };
        let found = match u16_at(table, 0) {
            Some(4) => format4(table, ch as u32),
            Some(12) => format12(table, ch as u32),
            _ => None,
        };
        if found == Some(true) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format4_finds_sorted_ranges_and_respects_subtable_length() {
        let words = [
            4u16, 44, 0, 6, 4, 1, 2, // 標頭
            0x43, 0x48, 0xffff, 0, // endCode 和 reservedPad
            0x41, 0x47, 0xffff, // startCode
            0xffc2, 1, 1, // idDelta
            0, 4, 0, // idRangeOffset
            6, 0, // glyphIdArray
        ];
        let mut table = words
            .into_iter()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        assert_eq!(format4(&table, 'A' as u32), Some(true));
        assert_eq!(format4(&table, 'C' as u32), Some(true));
        assert_eq!(format4(&table, 'D' as u32), Some(false));
        assert_eq!(format4(&table, 'G' as u32), Some(true));
        assert_eq!(format4(&table, 'H' as u32), Some(false));
        assert_eq!(format4(&table, 0xffff), Some(false));
        assert_eq!(format4(&table, '𠮷' as u32), None);

        // 下一個子表的位元組仍在切片中，但不屬於 format 4 的宣告長度。
        table[2..4].copy_from_slice(&40u16.to_be_bytes());
        assert_eq!(format4(&table, 'G' as u32), None);
        assert_eq!(format4(&table, 'C' as u32), Some(true));
    }
}
