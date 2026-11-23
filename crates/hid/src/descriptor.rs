//! HID report descriptor parsing: which HID++ reports a node declares.

/// `(has_short, has_long)`: whether the descriptor declares the HID++ report ids 0x10
/// and 0x11 while the usage page is a vendor page (>= 0xFF00, e.g. 0xFF00 or 0xFF43).
pub fn parse_report_ids(desc: &[u8]) -> (bool, bool) {
    let mut i = 0;
    let mut usage_page: u32 = 0;
    let (mut has_short, mut has_long) = (false, false);
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            // long item: 0xFE, size, tag, data
            let Some(&size) = desc.get(i + 1) else { break };
            i += 3 + size as usize;
            continue;
        }
        let size = [0usize, 1, 2, 4][(b & 0x03) as usize];
        let tag_type = b & 0xFC;
        let end = (i + 1 + size).min(desc.len());
        let data = desc[(i + 1).min(end)..end]
            .iter()
            .rev()
            .fold(0u32, |acc, &x| (acc << 8) | u32::from(x));
        match tag_type {
            0x04 => usage_page = data, // Usage Page
            0x84 if usage_page >= 0xFF00 => match data {
                0x10 => has_short = true,
                0x11 => has_long = true,
                _ => {}
            },
            _ => {}
        }
        i += 1 + size;
    }
    (has_short, has_long)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_report_ids() {
        // Usage Page (0xFF00), Report ID 0x10, Report ID 0x11
        let desc = [
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x10, 0x75, 0x08, 0x95, 0x06, 0x81,
            0x00, 0x85, 0x11, 0xC0,
        ];
        assert_eq!(parse_report_ids(&desc), (true, true));
        // A normal mouse descriptor with report id 0x11 on the generic desktop page.
        assert_eq!(parse_report_ids(&[0x05, 0x01, 0x85, 0x11]), (false, false));
        // Bluetooth HID++ page 0xFF43, long only.
        assert_eq!(
            parse_report_ids(&[0x06, 0x43, 0xFF, 0x0A, 0x02, 0x02, 0x85, 0x11]),
            (false, true)
        );
    }

    #[test]
    fn truncated_and_long_items_do_not_panic() {
        assert_eq!(parse_report_ids(&[0x06, 0x00]), (false, false));
        assert_eq!(parse_report_ids(&[0xFE]), (false, false));
        assert_eq!(parse_report_ids(&[0xFE, 0x02, 0x00, 0x01]), (false, false));
        assert_eq!(parse_report_ids(&[0x07, 0x00, 0xFF, 0x00]), (false, false));
    }
}
