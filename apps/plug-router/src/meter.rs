//! Common bounded metering-service contract.

use zigbee_plug_core::ElectricalSample;

/// Result of one bounded meter service call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterServiceOutcome {
    Idle,
    Sample(ElectricalSample),
    /// A pulse window was discarded because input capture overflowed.
    InputDropped,
    /// A serial meter interface was reset after a hardware I/O error.
    InterfaceReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterFault {
    StartupTimeout,
    Stale,
    InputDropped,
    InterfaceReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterSafetyState {
    AwaitingFirstSafeSample,
    Healthy,
    FaultLatched(MeterFault),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterHealthPolicy {
    pub startup_timeout_ms: u32,
    pub stale_timeout_ms: u32,
}

impl MeterHealthPolicy {
    pub const fn is_valid(self) -> bool {
        self.startup_timeout_ms != 0
            && self.stale_timeout_ms != 0
            && self.startup_timeout_ms <= i32::MAX as u32
            && self.stale_timeout_ms <= i32::MAX as u32
    }
}

impl Default for MeterHealthPolicy {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 10_000,
            stale_timeout_ms: 10_000,
        }
    }
}

/// Fitted meter service selected by the composition root.
pub trait MeterService {
    fn restore_energy_uwh(&mut self, total_uwh: u64);

    fn total_energy_uwh(&self) -> u64;

    /// Perform one bounded, nonblocking service operation.
    fn service(&mut self, now_ms: u32) -> MeterServiceOutcome;
}
