// SPDX-License-Identifier: GPL-3.0-or-later
//! Dense code storage and demand-zero heap pages.
use crate::{HorizonResult, shared_memory::SharedMemory};
use std::collections::BTreeMap;
const PAGE: usize = 4096;

#[derive(Debug, Clone)]
pub(super) enum Backing {
    Dense(Vec<u8>),
    Sparse(BTreeMap<usize, Box<[u8; PAGE]>>),
    Shared(SharedMemory),
}

impl Backing {
    pub fn read(&self, offset: usize, output: &mut [u8]) -> HorizonResult<()> {
        if let Self::Shared(memory) = self {
            return memory.read(offset, output);
        }
        if let Self::Dense(bytes) = self {
            output.copy_from_slice(&bytes[offset..offset + output.len()]);
            return Ok(());
        }
        let Self::Sparse(pages) = self else {
            unreachable!()
        };
        let mut done = 0;
        while done < output.len() {
            let position = offset + done;
            let within = position % PAGE;
            let count = (PAGE - within).min(output.len() - done);
            if let Some(page) = pages.get(&(position / PAGE)) {
                output[done..done + count].copy_from_slice(&page[within..within + count]);
            } else {
                output[done..done + count].fill(0);
            }
            done += count;
        }
        Ok(())
    }

    pub fn write(&mut self, offset: usize, input: &[u8]) -> HorizonResult<()> {
        if let Self::Shared(memory) = self {
            return memory.write(offset, input);
        }
        if let Self::Dense(bytes) = self {
            bytes[offset..offset + input.len()].copy_from_slice(input);
            return Ok(());
        }
        let Self::Sparse(pages) = self else {
            unreachable!()
        };
        let mut done = 0;
        while done < input.len() {
            let position = offset + done;
            let within = position % PAGE;
            let count = (PAGE - within).min(input.len() - done);
            let page = pages
                .entry(position / PAGE)
                .or_insert_with(|| Box::new([0; PAGE]));
            page[within..within + count].copy_from_slice(&input[done..done + count]);
            done += count;
        }
        Ok(())
    }

    pub fn shrink_heap(&mut self, size: usize) {
        let Self::Sparse(pages) = self else {
            unreachable!("heap backing must be sparse")
        };
        // Heap sizes are page aligned. Discarded pages must read zero after regrowth.
        pages.retain(|&index, _| index < size / PAGE);
    }
}
