// SPDX-License-Identifier: GPL-3.0-or-later
//! ARM64 bitmask decoding shared by logical-immediate and bitfield instructions.

fn ones(count: u32) -> u64 {
    if count == 64 {
        u64::MAX
    } else {
        (1u64 << count) - 1
    }
}

fn rotate(value: u64, amount: u32, width: u32) -> u64 {
    let amount = amount % width;
    if amount == 0 {
        value & ones(width)
    } else {
        ((value >> amount) | (value << (width - amount))) & ones(width)
    }
}

fn replicate(element: u64, element_width: u32, width: u32) -> u64 {
    let mut result = 0;
    for offset in (0..width).step_by(element_width as usize) {
        result |= element << offset;
    }
    result
}

/// Returns write and top masks, or None for architecturally reserved encodings.
pub(super) fn decode(
    n: u32,
    immr: u32,
    imms: u32,
    width: u32,
    logical: bool,
) -> Option<(u64, u64)> {
    let selector = (n << 6) | ((!imms) & 63);
    if selector == 0 {
        return None;
    }
    let length = 31 - selector.leading_zeros();
    if length < 1 {
        return None;
    }
    let element_width = 1u32 << length;
    if element_width > width {
        return None;
    }
    let levels = element_width - 1;
    let size = imms & levels;
    let rotation = immr & levels;
    if logical && size == levels {
        return None;
    }
    let difference = size.wrapping_sub(rotation) & levels;
    let write = rotate(ones(size + 1), rotation, element_width);
    let top = ones(difference + 1);
    Some((
        replicate(write, element_width, width),
        replicate(top, element_width, width),
    ))
}

pub(super) fn apply(
    source: u64,
    destination: u64,
    n: u32,
    immr: u32,
    imms: u32,
    width: u32,
    operation: u32,
) -> Option<u64> {
    if n != u32::from(width == 64) || immr >= width || imms >= width || operation == 3 {
        return None;
    }
    let (write, top) = decode(n, immr, imms, width, false)?;
    let rotated = rotate(source, immr, width);
    let value = match operation {
        0 => {
            let sign_fill = if source & (1u64 << imms) != 0 {
                ones(width)
            } else {
                0
            };
            (sign_fill & !top) | ((rotated & write) & top)
        }
        1 => {
            let bottom = (destination & !write) | (rotated & write);
            (destination & !top) | (bottom & top)
        }
        2 => rotated & write & top,
        _ => return None,
    };
    Some(value & ones(width))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_extension_and_shift_aliases() {
        assert_eq!(
            apply(0x80000000, 0, 1, 0, 31, 64, 0),
            Some(0xffffffff80000000)
        );
        assert_eq!(apply(0x80, 0, 1, 0, 7, 64, 0), Some(0xffffffffffffff80));
        assert_eq!(apply(0x12345678, 0, 0, 8, 31, 32, 2), Some(0x123456));
        assert_eq!(apply(3, 0, 1, 60, 59, 64, 2), Some(48));
    }
    #[test]
    fn insertion_preserves_surrounding_bits_and_reserved_forms_fail() {
        assert_eq!(apply(0xa, 0xffff, 1, 60, 3, 64, 1), Some(0xffaf));
        assert!(apply(0, 0, 1, 0, 31, 32, 0).is_none());
        assert!(decode(0, 0, 63, 64, true).is_none());
        assert_eq!(decode(1, 0, 7, 64, true).unwrap().0, 0xff);
    }
}
