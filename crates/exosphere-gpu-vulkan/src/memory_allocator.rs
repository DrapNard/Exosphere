// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Timeline-aware range suballocator used by Vulkan memory heaps and staging rings.
//! It deliberately owns no Vulkan handles: the same allocator can back device-local blocks,
//! persistent upload rings and readback rings.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
struct Retired {
    allocation: Allocation,
    reusable_after: u64,
}

#[derive(Debug)]
pub struct RangeAllocator {
    size: u64,
    free: Vec<Allocation>,
    retired: Vec<Retired>,
}

impl RangeAllocator {
    pub fn new(size: u64) -> Self {
        Self {
            size,
            free: vec![Allocation { offset: 0, size }],
            retired: Vec::new(),
        }
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn allocate(&mut self, size: u64, alignment: u64) -> Option<Allocation> {
        if size == 0 {
            return Some(Allocation { offset: 0, size: 0 });
        }
        let alignment = alignment.max(1);
        for i in 0..self.free.len() {
            let r = self.free[i];
            let aligned = r.offset.checked_add(alignment - 1)? / alignment * alignment;
            let end = aligned.checked_add(size)?;
            let rend = r.offset.checked_add(r.size)?;
            if end > rend {
                continue;
            }
            self.free.remove(i);
            if aligned > r.offset {
                self.free.push(Allocation {
                    offset: r.offset,
                    size: aligned - r.offset,
                })
            }
            if end < rend {
                self.free.push(Allocation {
                    offset: end,
                    size: rend - end,
                })
            }
            self.coalesce();
            return Some(Allocation {
                offset: aligned,
                size,
            });
        }
        None
    }
    pub fn retire(&mut self, allocation: Allocation, reusable_after: u64) {
        if allocation.size > 0 {
            self.retired.push(Retired {
                allocation,
                reusable_after,
            })
        }
    }
    pub fn collect(&mut self, completed: u64) {
        let mut keep = Vec::new();
        let retired = std::mem::take(&mut self.retired);
        for r in retired {
            if r.reusable_after <= completed {
                self.free.push(r.allocation)
            } else {
                keep.push(r)
            }
        }
        self.retired = keep;
        self.coalesce();
    }
    pub fn free_bytes(&self) -> u64 {
        self.free.iter().map(|r| r.size).sum()
    }
    fn coalesce(&mut self) {
        self.free.sort_by_key(|r| r.offset);
        let mut out: Vec<Allocation> = Vec::with_capacity(self.free.len());
        for r in self.free.drain(..) {
            if let Some(last) = out.last_mut() {
                if last.offset + last.size == r.offset {
                    last.size += r.size;
                    continue;
                }
            }
            out.push(r)
        }
        self.free = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_recycle() {
        let mut allocator = RangeAllocator::new(1024);
        let first = allocator.allocate(100, 256).unwrap();
        let second = allocator.allocate(100, 256).unwrap();
        assert_eq!((first.offset, second.offset), (0, 256));
        assert_eq!(allocator.free_bytes(), 824);
        allocator.retire(first, 7);
        allocator.collect(6);
        assert_eq!(allocator.free_bytes(), 824);
        // Until timeline 7 completes, the first range remains unavailable.
        let third = allocator.allocate(100, 256).unwrap();
        assert_eq!(third.offset, 512);
        allocator.collect(7);
        assert_eq!(allocator.allocate(100, 256).unwrap(), first);
    }
}
