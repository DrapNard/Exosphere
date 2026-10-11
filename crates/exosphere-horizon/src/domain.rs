// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded CMIF domain framing and per-session object ownership.

use crate::{
    error::{HorizonError, HorizonResult},
    kernel::SessionTarget,
};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct Domain {
    pub objects: BTreeMap<u32, SessionTarget>,
    next_id: u32,
}

impl Domain {
    pub fn new(root: SessionTarget) -> Self {
        Self {
            objects: BTreeMap::from([(1, root)]),
            next_id: 2,
        }
    }

    pub fn insert(&mut self, object: SessionTarget) -> HorizonResult<u32> {
        // Bound resource consumption independently of the native handle table.
        if self.objects.len() >= 0x4000 || self.next_id == u32::MAX {
            return Err(HorizonError::InvalidState(
                "domain object table exhausted".into(),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.objects.insert(id, object);
        Ok(id)
    }
}

pub struct DomainRequest<'a> {
    pub release: bool,
    pub target: u32,
    pub cmif: &'a [u8],
}

pub fn parse(raw: &[u8], front_padding: usize) -> HorizonResult<DomainRequest<'_>> {
    let header = raw
        .get(front_padding..front_padding + 16)
        .ok_or_else(|| protocol("truncated domain header"))?;
    let size = u16::from_le_bytes(header[2..4].try_into().unwrap()) as usize;
    let count = header[1] as usize;
    let target = u32::from_le_bytes(header[4..8].try_into().unwrap());
    if target == 0 || header[8..12] != [0; 4] {
        return Err(protocol("invalid domain target or reserved field"));
    }
    let start = front_padding + 16;
    let cmif = raw
        .get(start..start + size)
        .ok_or_else(|| protocol("truncated domain body"))?;
    raw.get(start + size..start + size + count * 4)
        .ok_or_else(|| protocol("truncated domain input-object array"))?;
    // Input interface transfer requires service-level type/ownership validation.
    // Reject it explicitly until those callbacks accept typed input objects.
    if count != 0 {
        return Err(protocol("domain input objects are not implemented"));
    }
    let release = match header[0] {
        1 if size >= 16 => false,
        2 if size == 0 => true,
        _ => return Err(protocol("invalid domain request kind/body size")),
    };
    Ok(DomainRequest {
        release,
        target,
        cmif,
    })
}

pub fn encode(cmif: &[u8], objects: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0; 16];
    bytes[..4].copy_from_slice(&(objects.len() as u32).to_le_bytes());
    bytes.extend_from_slice(cmif);
    for object in objects {
        bytes.extend_from_slice(&object.to_le_bytes());
    }
    bytes
}

fn protocol(message: &str) -> HorizonError {
    HorizonError::IpcProtocol(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_framing_is_bounded_and_ids_are_not_reused() {
        let mut raw = vec![0; 8 + 16 + 16];
        raw[8] = 1;
        raw[10..12].copy_from_slice(&16u16.to_le_bytes());
        raw[12..16].copy_from_slice(&1u32.to_le_bytes());
        raw[24..28].copy_from_slice(b"SFCI");
        let request = parse(&raw, 8).unwrap();
        assert_eq!(request.target, 1);
        assert_eq!(&request.cmif[..4], b"SFCI");
        assert!(parse(&raw[..39], 8).is_err());
        raw[9] = 1;
        assert!(parse(&raw, 8).is_err());
        let mut domain = Domain::new(SessionTarget::ApmSession);
        assert_eq!(domain.insert(SessionTarget::ApmSession).unwrap(), 2);
        domain.objects.remove(&2);
        assert_eq!(domain.insert(SessionTarget::ApmSession).unwrap(), 3);
        let encoded = encode(b"SFCO", &[3]);
        assert_eq!(&encoded[..4], &1u32.to_le_bytes());
        assert_eq!(&encoded[16..20], b"SFCO");
        assert_eq!(&encoded[20..24], &3u32.to_le_bytes());
    }
}
