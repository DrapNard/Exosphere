// SPDX-License-Identifier: GPL-3.0-or-later
//! Checked Switch Binder parcel framing and aligned payload reads.

use crate::{HorizonError, HorizonResult};

const HEADER_SIZE: usize = 16;
/// Explicit host staging limit, not a claimed firmware-wide maximum.
const MAX_SIZE: usize = 1024 * 1024;

fn malformed() -> HorizonError {
    HorizonError::IpcProtocol("invalid or truncated Binder parcel".into())
}

#[derive(Debug)]
pub struct Parcel<'a> {
    pub payload: &'a [u8],
    /// Opaque object-table bytes. Object ownership is validated by the Binder registry.
    pub objects: &'a [u8],
}

impl<'a> Parcel<'a> {
    pub fn parse(bytes: &'a [u8]) -> HorizonResult<Self> {
        if bytes.len() < HEADER_SIZE || bytes.len() > MAX_SIZE {
            return Err(malformed());
        }
        let word =
            |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let range = |offset: usize, size: usize| -> HorizonResult<std::ops::Range<usize>> {
            let end = offset
                .checked_add(size)
                .filter(|&end| end <= bytes.len())
                .ok_or_else(malformed)?;
            if size != 0 && (offset < HEADER_SIZE || offset % 4 != 0) {
                return Err(malformed());
            }
            Ok(offset..end)
        };
        let payload = range(word(4), word(0))?;
        let objects = range(word(12), word(8))?;
        if !payload.is_empty()
            && !objects.is_empty()
            && payload.start < objects.end
            && objects.start < payload.end
        {
            return Err(malformed());
        }
        Ok(Self {
            payload: &bytes[payload],
            objects: &bytes[objects],
        })
    }

    pub fn reader(&self) -> ParcelReader<'a> {
        ParcelReader {
            payload: self.payload,
            position: 0,
        }
    }
}

/// Serializes a payload with an explicitly supplied object table; no fake objects.
pub fn encode(payload: &[u8], objects: &[u8]) -> HorizonResult<Vec<u8>> {
    let objects_offset = HEADER_SIZE
        .checked_add(payload.len())
        .ok_or_else(malformed)?;
    let size = objects_offset
        .checked_add(objects.len())
        .filter(|&size| size <= MAX_SIZE)
        .ok_or_else(malformed)?;
    if payload.len() % 4 != 0 {
        return Err(malformed());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| HorizonError::InvalidState("parcel allocation failed".into()))?;
    for word in [
        payload.len() as u32,
        HEADER_SIZE as u32,
        objects.len() as u32,
        objects_offset as u32,
    ] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(objects);
    Ok(bytes)
}

#[derive(Debug, Clone)]
pub struct ParcelReader<'a> {
    payload: &'a [u8],
    position: usize,
}

impl<'a> ParcelReader<'a> {
    pub fn position(&self) -> usize {
        self.position
    }

    pub fn read(&mut self, size: usize) -> HorizonResult<&'a [u8]> {
        let aligned = size.checked_add(3).ok_or_else(malformed)? & !3;
        let end = self
            .position
            .checked_add(aligned)
            .filter(|&end| end <= self.payload.len())
            .ok_or_else(malformed)?;
        let data = &self.payload[self.position..self.position + size];
        self.position = end;
        Ok(data)
    }

    pub fn read_u32(&mut self) -> HorizonResult<u32> {
        Ok(u32::from_le_bytes(self.read(4)?.try_into().unwrap()))
    }

    /// Failures leave the cursor untouched, including malformed UTF-16/terminators.
    pub fn read_string16(&mut self) -> HorizonResult<String> {
        let mut reader = self.clone();
        let length = reader.read_u32()? as i32;
        if length < 0 {
            return Err(malformed());
        }
        let size = (length as usize)
            .checked_add(1)
            .and_then(|length| length.checked_mul(2))
            .ok_or_else(malformed)?;
        let bytes = reader.read(size)?;
        if bytes[size - 2..] != [0, 0] {
            return Err(malformed());
        }
        let units: Vec<u16> = bytes[..size - 2]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes(pair.try_into().unwrap()))
            .collect();
        let value = String::from_utf16(&units).map_err(|_| malformed())?;
        self.position = reader.position;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip_exposes_distinct_payload_and_object_table() {
        // Independent wire fixture: four little-endian header words, then payload/table.
        let fixture = [
            4, 0, 0, 0, 16, 0, 0, 0, 4, 0, 0, 0, 20, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,
        ];
        let parcel = Parcel::parse(&fixture).unwrap();
        assert_eq!(parcel.payload, [7, 0, 0, 0]);
        assert_eq!(parcel.objects, [0, 0, 0, 0]);
        assert_eq!(encode(parcel.payload, parcel.objects).unwrap(), fixture);
        for length in 0..fixture.len() {
            assert!(Parcel::parse(&fixture[..length]).is_err());
        }
        for (offset, word) in [(0, u32::MAX), (4, 4), (4, 17), (8, u32::MAX), (12, 16)] {
            let mut invalid = fixture;
            invalid[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
            assert!(Parcel::parse(&invalid).is_err());
        }
    }

    #[test]
    fn aligned_reads_and_string_failures_preserve_cursor() {
        let payload = [2, 0, 0, 0, b'A', 0, b'B', 0, 0, 0, 0, 0, 42, 0, 0, 0];
        let bytes = encode(&payload, &[]).unwrap();
        let parcel = Parcel::parse(&bytes).unwrap();
        let mut reader = parcel.reader();
        assert_eq!(reader.read_string16().unwrap(), "AB");
        assert_eq!(reader.position(), 12);
        assert_eq!(reader.read_u32().unwrap(), 42);
        assert!(reader.read(usize::MAX).is_err());
        assert_eq!(reader.position(), 16);
        for invalid in [
            vec![255; 4],
            vec![1, 0, 0, 0, 0, 216, 0, 0],
            vec![1, 0, 0, 0, b'A', 0, 1, 0],
        ] {
            let bytes = encode(&invalid, &[]).unwrap();
            let parcel = Parcel::parse(&bytes).unwrap();
            let mut reader = parcel.reader();
            assert!(reader.read_string16().is_err());
            assert_eq!(reader.position(), 0);
        }
    }
}
