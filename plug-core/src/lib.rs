//! Hardware-independent smart-plug behavior.

#![no_std]

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ElectricalSample {
    pub voltage_mv: u32,
    pub current_ma: u32,
    pub active_power_mw: i32,
    pub frequency_millihz: Option<u32>,
    pub total_energy_uwh: u64,
}

impl ElectricalSample {
    pub const fn total_energy_wh(self) -> u64 {
        self.total_energy_uwh / 1_000_000
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupBehavior {
    Off,
    On,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedMode {
    Off,
    On,
    FollowRelay,
}

impl LedMode {
    pub const fn led_on(self, relay_on: bool) -> bool {
        match self {
            Self::Off => false,
            Self::On => true,
            Self::FollowRelay => relay_on,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlugSettings {
    pub startup_behavior: StartupBehavior,
    pub led_mode: LedMode,
    pub button_locked: bool,
}

impl Default for PlugSettings {
    fn default() -> Self {
        Self {
            startup_behavior: StartupBehavior::Previous,
            led_mode: LedMode::FollowRelay,
            button_locked: false,
        }
    }
}

impl PlugSettings {
    pub const fn startup_relay(self, previous: bool) -> bool {
        match self.startup_behavior {
            StartupBehavior::Off => false,
            StartupBehavior::On => true,
            StartupBehavior::Previous => previous,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripReason {
    UnderVoltage,
    OverVoltage,
    OverCurrent,
    OverPower,
}

impl TripReason {
    const fn is_voltage(self) -> bool {
        matches!(self, Self::UnderVoltage | Self::OverVoltage)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtectionConfig {
    /// Zero disables the lower-voltage threshold.
    pub min_voltage_mv: u32,
    /// Zero disables the upper-voltage threshold.
    pub max_voltage_mv: u32,
    /// Zero disables current protection.
    pub max_current_ma: u32,
    /// Zero disables power protection.
    pub max_power_mw: u32,
    pub trip_delay_ms: u32,
    pub auto_restart_voltage: bool,
    pub restart_delay_ms: u32,
}

impl Default for ProtectionConfig {
    fn default() -> Self {
        Self {
            min_voltage_mv: 180_000,
            max_voltage_mv: 250_000,
            max_current_ma: 16_000,
            max_power_mw: 3_680_000,
            trip_delay_ms: 5_000,
            auto_restart_voltage: false,
            restart_delay_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionAction {
    None,
    Trip(TripReason),
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtectionEngine {
    config: ProtectionConfig,
    pending: Option<TripReason>,
    pending_since_ms: u32,
    tripped: Option<TripReason>,
    safe_since_ms: Option<u32>,
    restore_relay: bool,
}

impl ProtectionEngine {
    pub const fn new(config: ProtectionConfig) -> Self {
        Self {
            config,
            pending: None,
            pending_since_ms: 0,
            tripped: None,
            safe_since_ms: None,
            restore_relay: false,
        }
    }

    pub const fn trip_reason(&self) -> Option<TripReason> {
        self.tripped
    }

    pub const fn sample_violation(&self, sample: ElectricalSample) -> Option<TripReason> {
        self.violation(sample)
    }

    pub fn clear_latch(&mut self) {
        self.pending = None;
        self.tripped = None;
        self.safe_since_ms = None;
        self.restore_relay = false;
    }

    pub fn update(
        &mut self,
        now_ms: u32,
        sample: ElectricalSample,
        relay_was_on: bool,
    ) -> ProtectionAction {
        let violation = self.violation(sample);

        if let Some(tripped) = self.tripped {
            if violation.is_some() {
                self.safe_since_ms = None;
                return ProtectionAction::None;
            }
            if !tripped.is_voltage() || !self.config.auto_restart_voltage || !self.restore_relay {
                return ProtectionAction::None;
            }

            let safe_since = *self.safe_since_ms.get_or_insert(now_ms);
            if elapsed(now_ms, safe_since) >= self.config.restart_delay_ms {
                self.clear_latch();
                return ProtectionAction::Restart;
            }
            return ProtectionAction::None;
        }

        let Some(reason) = violation else {
            self.pending = None;
            return ProtectionAction::None;
        };

        if self.pending != Some(reason) {
            self.pending = Some(reason);
            self.pending_since_ms = now_ms;
            return ProtectionAction::None;
        }

        if elapsed(now_ms, self.pending_since_ms) < self.config.trip_delay_ms {
            return ProtectionAction::None;
        }

        self.pending = None;
        self.tripped = Some(reason);
        self.restore_relay = relay_was_on;
        ProtectionAction::Trip(reason)
    }

    const fn violation(&self, sample: ElectricalSample) -> Option<TripReason> {
        if self.config.min_voltage_mv != 0 && sample.voltage_mv < self.config.min_voltage_mv {
            return Some(TripReason::UnderVoltage);
        }
        if self.config.max_voltage_mv != 0 && sample.voltage_mv > self.config.max_voltage_mv {
            return Some(TripReason::OverVoltage);
        }
        if self.config.max_current_ma != 0 && sample.current_ma > self.config.max_current_ma {
            return Some(TripReason::OverCurrent);
        }
        if self.config.max_power_mw != 0
            && sample.active_power_mw.unsigned_abs() > self.config.max_power_mw
        {
            return Some(TripReason::OverPower);
        }
        None
    }
}

const fn elapsed(now: u32, since: u32) -> u32 {
    now.wrapping_sub(since)
}

/// Extends a wrapping hardware tick counter into a wrapping millisecond clock.
///
/// Call [`Self::update`] at least once per full wrap of the source counter.
/// The fractional-tick remainder is retained, so repeated sub-millisecond
/// updates do not accumulate truncation drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickMillis {
    ticks_per_ms: u32,
    last_ticks: u32,
    remainder_ticks: u32,
    millis: u32,
}

impl TickMillis {
    pub const fn new(ticks_per_ms: u32, initial_ticks: u32) -> Option<Self> {
        if ticks_per_ms == 0 {
            return None;
        }
        Some(Self {
            ticks_per_ms,
            last_ticks: initial_ticks,
            remainder_ticks: 0,
            millis: 0,
        })
    }

    pub const fn millis(&self) -> u32 {
        self.millis
    }

    pub fn update(&mut self, ticks: u32) -> u32 {
        let elapsed_ticks = ticks.wrapping_sub(self.last_ticks);
        self.last_ticks = ticks;

        let total_ticks = u64::from(self.remainder_ticks) + u64::from(elapsed_ticks);
        let ticks_per_ms = u64::from(self.ticks_per_ms);
        let elapsed_ms = total_ticks / ticks_per_ms;
        self.remainder_ticks = (total_ticks % ticks_per_ms) as u32;
        self.millis = self.millis.wrapping_add(elapsed_ms as u32);
        self.millis
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    InvalidChecksum,
}

/// Sixteen-byte wear-journal record: sequence, energy in mWh, CRC-32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergyRecord {
    pub sequence: u32,
    pub energy_mwh: u64,
}

impl EnergyRecord {
    pub const LEN: usize = 16;

    pub const fn new(sequence: u32, energy_mwh: u64) -> Self {
        Self {
            sequence,
            energy_mwh,
        }
    }

    pub fn encode(self) -> [u8; Self::LEN] {
        let mut bytes = [0u8; Self::LEN];
        bytes[0..4].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[4..12].copy_from_slice(&self.energy_mwh.to_le_bytes());
        let checksum = crc32(&bytes[..12]);
        bytes[12..16].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: [u8; Self::LEN]) -> Result<Self, RecordError> {
        let expected = u32::from_le_bytes(bytes[12..16].try_into().unwrap_or([0; 4]));
        if crc32(&bytes[..12]) != expected {
            return Err(RecordError::InvalidChecksum);
        }
        Ok(Self {
            sequence: u32::from_le_bytes(bytes[0..4].try_into().unwrap_or([0; 4])),
            energy_mwh: u64::from_le_bytes(bytes[4..12].try_into().unwrap_or([0; 8])),
        })
    }
}

/// Wear-journal record persisting the controller's relay state and
/// monotonic lifetime energy across reboots.
///
/// Distinct from [`EnergyRecord`]: this stores full microwatt-hour
/// precision (matching `bl0942::EnergyTracker`/`bl0937::Bl0937`'s native
/// `total_uwh()` unit) instead of rounding to milliwatt-hours, and also
/// carries the relay's last commanded state so `StartupBehavior::Previous`
/// has a real value to restore. `sequence` lets a product's application-NV
/// store (`LogStructuredNv`) detect the newest of several wear-leveled
/// copies the same way [`EnergyRecord`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlugStateRecord {
    pub sequence: u32,
    pub relay_on: bool,
    pub energy_uwh: u64,
}

impl PlugStateRecord {
    pub const LEN: usize = 17;

    pub const fn new(sequence: u32, relay_on: bool, energy_uwh: u64) -> Self {
        Self {
            sequence,
            relay_on,
            energy_uwh,
        }
    }

    pub fn encode(self) -> [u8; Self::LEN] {
        let mut bytes = [0u8; Self::LEN];
        bytes[0..4].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[4] = u8::from(self.relay_on);
        bytes[5..13].copy_from_slice(&self.energy_uwh.to_le_bytes());
        let checksum = crc32(&bytes[..13]);
        bytes[13..17].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: [u8; Self::LEN]) -> Result<Self, RecordError> {
        let expected = u32::from_le_bytes(bytes[13..17].try_into().unwrap_or([0; 4]));
        if crc32(&bytes[..13]) != expected {
            return Err(RecordError::InvalidChecksum);
        }
        Ok(Self {
            sequence: u32::from_le_bytes(bytes[0..4].try_into().unwrap_or([0; 4])),
            relay_on: bytes[4] != 0,
            energy_uwh: u64::from_le_bytes(bytes[5..13].try_into().unwrap_or([0; 8])),
        })
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nominal() -> ElectricalSample {
        ElectricalSample {
            voltage_mv: 230_000,
            current_ma: 1_000,
            active_power_mw: 200_000,
            frequency_millihz: Some(50_000),
            total_energy_uwh: 1_500_000,
        }
    }

    #[test]
    fn voltage_trip_requires_sustained_violation_and_can_restart() {
        let mut engine = ProtectionEngine::new(ProtectionConfig {
            trip_delay_ms: 5_000,
            auto_restart_voltage: true,
            restart_delay_ms: 10_000,
            ..ProtectionConfig::default()
        });
        let mut over = nominal();
        over.voltage_mv = 260_000;
        assert_eq!(engine.update(1_000, over, true), ProtectionAction::None);
        assert_eq!(
            engine.update(6_000, over, true),
            ProtectionAction::Trip(TripReason::OverVoltage)
        );
        assert_eq!(
            engine.update(7_000, nominal(), false),
            ProtectionAction::None
        );
        assert_eq!(
            engine.update(17_000, nominal(), false),
            ProtectionAction::Restart
        );
        assert_eq!(engine.trip_reason(), None);
    }

    #[test]
    fn current_trip_stays_latched() {
        let mut engine = ProtectionEngine::new(ProtectionConfig {
            trip_delay_ms: 0,
            auto_restart_voltage: true,
            ..ProtectionConfig::default()
        });
        let mut overload = nominal();
        overload.current_ma = 20_000;
        assert_eq!(engine.update(10, overload, true), ProtectionAction::None);
        assert_eq!(
            engine.update(10, overload, true),
            ProtectionAction::Trip(TripReason::OverCurrent)
        );
        assert_eq!(
            engine.update(100_000, nominal(), false),
            ProtectionAction::None
        );
    }

    #[test]
    fn settings_apply_startup_and_led_policy() {
        let settings = PlugSettings {
            startup_behavior: StartupBehavior::On,
            led_mode: LedMode::FollowRelay,
            button_locked: true,
        };
        assert!(settings.startup_relay(false));
        assert!(settings.led_mode.led_on(true));
        assert!(!settings.led_mode.led_on(false));
    }

    #[test]
    fn tick_millis_preserves_fractional_ticks() {
        let mut clock = TickMillis::new(1_000, 100).unwrap();
        assert_eq!(clock.update(600), 0);
        assert_eq!(clock.update(1_099), 0);
        assert_eq!(clock.update(1_100), 1);
        assert_eq!(clock.millis(), 1);
    }

    #[test]
    fn tick_millis_extends_a_wrapping_source_counter() {
        let initial = u32::MAX - 12_000;
        let mut clock = TickMillis::new(24_000, initial).unwrap();
        assert_eq!(clock.update(11_999), 1);
        assert_eq!(clock.update(35_999), 2);
    }

    #[test]
    fn energy_record_round_trips_and_detects_corruption() {
        let record = EnergyRecord::new(42, 9_876_543);
        let encoded = record.encode();
        assert_eq!(EnergyRecord::decode(encoded), Ok(record));

        let mut corrupt = encoded;
        corrupt[5] ^= 0x80;
        assert_eq!(
            EnergyRecord::decode(corrupt),
            Err(RecordError::InvalidChecksum)
        );
    }

    #[test]
    fn plug_state_record_round_trips_relay_and_full_precision_energy() {
        let record = PlugStateRecord::new(7, true, 1_234_567_890_123);
        let encoded = record.encode();
        assert_eq!(PlugStateRecord::decode(encoded), Ok(record));

        let off = PlugStateRecord::new(8, false, 0);
        assert_eq!(PlugStateRecord::decode(off.encode()), Ok(off));
    }

    #[test]
    fn plug_state_record_detects_corruption() {
        let record = PlugStateRecord::new(1, true, 42);
        let mut corrupt = record.encode();
        corrupt[9] ^= 0x01;
        assert_eq!(
            PlugStateRecord::decode(corrupt),
            Err(RecordError::InvalidChecksum)
        );
    }
}
