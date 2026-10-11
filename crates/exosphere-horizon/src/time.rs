// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Horizon-facing time model.
//!
//! Time sources are injectable so deterministic tests/replays do not depend on the host wall clock.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Horizon's architectural system-counter frequency, independent of host clocks.
pub const SYSTEM_COUNTER_FREQUENCY: u64 = 19_200_000;

pub trait TimeSource: Send + Sync {
    fn monotonic_ns(&self) -> u64;
    fn unix_seconds(&self) -> i64;
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixedTime(u64);
    impl TimeSource for FixedTime {
        fn monotonic_ns(&self) -> u64 {
            self.0
        }
        fn unix_seconds(&self) -> i64 {
            0
        }
    }

    #[test]
    fn architectural_ticks_use_exact_frequency_without_intermediate_overflow() {
        for (nanos, ticks) in [
            (0, 0),
            (52, 0),
            (53, 1),
            (1_000_000_000, 19_200_000),
            (u64::MAX, 354_177_486_215_223_391),
        ] {
            let time = TimeManager::new(Box::new(FixedTime(nanos)));
            assert_eq!(time.system_counter(), ticks);
            assert_eq!(time.steady_clock().time_point_ns, nanos);
        }
    }
}

#[derive(Debug)]
pub struct HostTimeSource {
    boot: Instant,
}

impl Default for HostTimeSource {
    fn default() -> Self {
        Self {
            boot: Instant::now(),
        }
    }
}

impl TimeSource for HostTimeSource {
    fn monotonic_ns(&self) -> u64 {
        self.boot.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
    }

    fn unix_seconds(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(value) => value.as_secs().min(i64::MAX as u64) as i64,
            Err(error) => -(error.duration().as_secs().min(i64::MAX as u64) as i64),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteadyClockTimePoint {
    pub time_point_ns: u64,
    pub source_id: [u8; 16],
}

pub struct TimeManager {
    source: Box<dyn TimeSource>,
    source_id: [u8; 16],
    timezone_offset_seconds: i32,
}

impl std::fmt::Debug for TimeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeManager")
            .field("source_id", &self.source_id)
            .field("timezone_offset_seconds", &self.timezone_offset_seconds)
            .finish_non_exhaustive()
    }
}

impl Default for TimeManager {
    fn default() -> Self {
        Self::new(Box::new(HostTimeSource::default()))
    }
}

impl TimeManager {
    pub fn new(source: Box<dyn TimeSource>) -> Self {
        Self {
            source,
            source_id: *b"EXOSPHERE-TIME\0\0",
            timezone_offset_seconds: 0,
        }
    }

    pub fn steady_clock(&self) -> SteadyClockTimePoint {
        SteadyClockTimePoint {
            time_point_ns: self.source.monotonic_ns(),
            source_id: self.source_id,
        }
    }

    /// Derive CPU ticks from the same monotonic source used for kernel deadlines.
    pub fn system_counter(&self) -> u64 {
        // Widen before multiplication: even a saturated nanosecond source must
        // not overflow during conversion. Fractional ticks are rounded down.
        (u128::from(self.source.monotonic_ns()) * u128::from(SYSTEM_COUNTER_FREQUENCY)
            / 1_000_000_000) as u64
    }

    pub fn unix_seconds(&self) -> i64 {
        self.source.unix_seconds()
    }
    pub fn timezone_offset_seconds(&self) -> i32 {
        self.timezone_offset_seconds
    }
}
