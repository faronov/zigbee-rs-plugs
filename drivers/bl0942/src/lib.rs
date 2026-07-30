//! BL0942 UART protocol and fixed-point measurement conversion.
//!
//! The transport is deliberately abstract: firmware writes command arrays and
//! feeds received bytes into [`StreamParser`].

#![no_std]

pub const READ_COMMAND: u8 = 0x58;
pub const WRITE_COMMAND: u8 = 0xA8;
pub const FULL_PACKET_REGISTER: u8 = 0xAA;
pub const PACKET_HEADER: u8 = 0x55;
pub const FRAME_LEN: usize = 23;
pub const COUNTER_MODULUS: u32 = 1 << 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    InvalidAddress,
    ZeroReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    BadHeader,
    BadChecksum { expected: u8, actual: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFrame {
    pub current_rms: u32,
    pub voltage_rms: u32,
    pub current_fast_rms: u32,
    pub active_power: i32,
    pub energy_counter: u32,
    pub frequency_period: u16,
    pub status: u8,
}

impl RawFrame {
    pub fn parse(bytes: &[u8; FRAME_LEN], address: u8) -> Result<Self, ParseError> {
        if bytes[0] != PACKET_HEADER {
            return Err(ParseError::BadHeader);
        }
        let expected = frame_checksum(address, &bytes[..FRAME_LEN - 1]);
        let actual = bytes[FRAME_LEN - 1];
        if expected != actual {
            return Err(ParseError::BadChecksum { expected, actual });
        }

        Ok(Self {
            current_rms: read_u24(&bytes[1..4]),
            voltage_rms: read_u24(&bytes[4..7]),
            current_fast_rms: read_u24(&bytes[7..10]),
            active_power: read_i24(&bytes[10..13]),
            energy_counter: read_u24(&bytes[13..16]),
            frequency_period: u16::from_le_bytes([bytes[16], bytes[17]]),
            status: bytes[19],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calibration {
    pub voltage_counts_per_volt: u32,
    pub current_counts_per_amp: u32,
    pub power_counts_per_watt: u32,
    pub energy_counts_per_kwh: u32,
}

impl Calibration {
    pub const fn new(
        voltage_counts_per_volt: u32,
        current_counts_per_amp: u32,
        power_counts_per_watt: u32,
        energy_counts_per_kwh: u32,
    ) -> Result<Self, ConfigError> {
        if voltage_counts_per_volt == 0
            || current_counts_per_amp == 0
            || power_counts_per_watt == 0
            || energy_counts_per_kwh == 0
        {
            return Err(ConfigError::ZeroReference);
        }
        Ok(Self {
            voltage_counts_per_volt,
            current_counts_per_amp,
            power_counts_per_watt,
            energy_counts_per_kwh,
        })
    }

    /// Reference values for a common 1 mOhm shunt and 5x390 kOhm/510 Ohm
    /// voltage divider. Production devices should be calibrated individually.
    pub const fn common_reference_board() -> Self {
        Self {
            voltage_counts_per_volt: 15_883,
            current_counts_per_amp: 251_066,
            power_counts_per_watt: 623,
            energy_counts_per_kwh: 5_347,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measurement {
    pub voltage_mv: u32,
    pub current_ma: u32,
    pub active_power_mw: i32,
    pub frequency_millihz: Option<u32>,
    /// Energy represented by the current 24-bit chip counter value.
    ///
    /// This wraps with the hardware counter and is not a lifetime total.
    /// Use [`EnergyTracker`] for monotonic, persistable energy.
    pub counter_energy_uwh: u64,
    pub status: u8,
}

impl Measurement {
    pub fn from_raw(frame: RawFrame, calibration: Calibration) -> Self {
        Self {
            voltage_mv: scale_unsigned(
                frame.voltage_rms,
                1_000,
                calibration.voltage_counts_per_volt,
            ),
            current_ma: scale_unsigned(
                frame.current_rms,
                1_000,
                calibration.current_counts_per_amp,
            ),
            active_power_mw: scale_signed(
                frame.active_power,
                1_000,
                calibration.power_counts_per_watt,
            ),
            frequency_millihz: (frame.frequency_period != 0).then(|| {
                (1_000_000_000u64 / u64::from(frame.frequency_period)).min(u64::from(u32::MAX))
                    as u32
            }),
            counter_energy_uwh: (u128::from(frame.energy_counter) * 1_000_000_000u128
                / u128::from(calibration.energy_counts_per_kwh))
            .min(u128::from(u64::MAX)) as u64,
            status: frame.status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedResult {
    Pending,
    Frame(RawFrame),
    Error(ParseError),
}

pub struct StreamParser {
    address: u8,
    buffer: [u8; FRAME_LEN],
    len: usize,
}

impl StreamParser {
    pub fn new(address: u8) -> Result<Self, ConfigError> {
        validate_address(address)?;
        Ok(Self {
            address,
            buffer: [0; FRAME_LEN],
            len: 0,
        })
    }

    pub fn push(&mut self, byte: u8) -> FeedResult {
        if self.len == 0 {
            if byte == PACKET_HEADER {
                self.buffer[0] = byte;
                self.len = 1;
            }
            return FeedResult::Pending;
        }

        self.buffer[self.len] = byte;
        self.len += 1;
        if self.len != FRAME_LEN {
            return FeedResult::Pending;
        }

        let frame = self.buffer;
        match RawFrame::parse(&frame, self.address) {
            Ok(frame) => {
                self.len = 0;
                FeedResult::Frame(frame)
            }
            Err(error) => {
                self.resynchronize();
                FeedResult::Error(error)
            }
        }
    }

    pub const fn buffered_len(&self) -> usize {
        self.len
    }

    fn resynchronize(&mut self) {
        let next_header = self.buffer[1..]
            .iter()
            .position(|byte| *byte == PACKET_HEADER)
            .map(|offset| offset + 1);
        if let Some(start) = next_header {
            let remaining = FRAME_LEN - start;
            self.buffer.copy_within(start..FRAME_LEN, 0);
            self.len = remaining;
        } else {
            self.len = 0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterEvent {
    First,
    Advanced(u32),
    Wrapped(u32),
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergyTracker {
    previous: Option<u32>,
    energy_counts_per_kwh: u32,
    remainder: u128,
    total_uwh: u64,
}

impl EnergyTracker {
    pub const fn new(energy_counts_per_kwh: u32) -> Result<Self, ConfigError> {
        if energy_counts_per_kwh == 0 {
            return Err(ConfigError::ZeroReference);
        }
        Ok(Self {
            previous: None,
            energy_counts_per_kwh,
            remainder: 0,
            total_uwh: 0,
        })
    }

    pub fn observe(&mut self, counter: u32) -> CounterEvent {
        let counter = counter & (COUNTER_MODULUS - 1);
        let Some(previous) = self.previous.replace(counter) else {
            return CounterEvent::First;
        };

        if counter >= previous {
            let delta = counter - previous;
            self.add_counts(delta);
            return CounterEvent::Advanced(delta);
        }

        if previous >= 0xF0_0000 && counter <= 0x0F_FFFF {
            let delta = COUNTER_MODULUS - previous + counter;
            self.add_counts(delta);
            CounterEvent::Wrapped(delta)
        } else {
            CounterEvent::Reset
        }
    }

    pub const fn total_uwh(&self) -> u64 {
        self.total_uwh
    }

    /// Restore the durable lifetime total and re-baseline the hardware
    /// counter on the next sample.
    pub fn restore_total_uwh(&mut self, total_uwh: u64) {
        self.previous = None;
        self.remainder = 0;
        self.total_uwh = total_uwh;
    }

    fn add_counts(&mut self, counts: u32) {
        let numerator = self.remainder + u128::from(counts) * 1_000_000_000u128;
        let divisor = u128::from(self.energy_counts_per_kwh);
        let delta_uwh = numerator / divisor;
        self.remainder = numerator % divisor;
        self.total_uwh = self
            .total_uwh
            .saturating_add(delta_uwh.min(u128::from(u64::MAX)) as u64);
    }
}

pub fn full_packet_request(address: u8) -> Result<[u8; 2], ConfigError> {
    validate_address(address)?;
    Ok([READ_COMMAND | address, FULL_PACKET_REGISTER])
}

pub fn read_register_request(address: u8, register: u8) -> Result<[u8; 2], ConfigError> {
    validate_address(address)?;
    Ok([READ_COMMAND | address, register])
}

pub fn write_register_request(
    address: u8,
    register: u8,
    value: u32,
) -> Result<[u8; 6], ConfigError> {
    validate_address(address)?;
    let mut packet = [
        WRITE_COMMAND | address,
        register,
        value as u8,
        (value >> 8) as u8,
        (value >> 16) as u8,
        0,
    ];
    packet[5] = checksum(&packet[..5]);
    Ok(packet)
}

pub fn frame_checksum(address: u8, bytes_without_checksum: &[u8]) -> u8 {
    let seed = READ_COMMAND | address;
    bytes_without_checksum
        .iter()
        .fold(seed, |sum, byte| sum.wrapping_add(*byte))
        ^ 0xFF
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) ^ 0xFF
}

const fn validate_address(address: u8) -> Result<(), ConfigError> {
    if address <= 3 {
        Ok(())
    } else {
        Err(ConfigError::InvalidAddress)
    }
}

fn read_u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn read_i24(bytes: &[u8]) -> i32 {
    let value = read_u24(bytes);
    if value & 0x80_0000 != 0 {
        (value | 0xFF00_0000) as i32
    } else {
        value as i32
    }
}

fn scale_unsigned(value: u32, multiplier: u32, divisor: u32) -> u32 {
    (u128::from(value) * u128::from(multiplier) / u128::from(divisor)).min(u128::from(u32::MAX))
        as u32
}

fn scale_signed(value: i32, multiplier: i32, divisor: u32) -> i32 {
    let scaled = i128::from(value) * i128::from(multiplier) / i128::from(divisor);
    scaled.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u24(bytes: &mut [u8], value: u32) {
        bytes[0] = value as u8;
        bytes[1] = (value >> 8) as u8;
        bytes[2] = (value >> 16) as u8;
    }

    fn sample_frame(address: u8) -> [u8; FRAME_LEN] {
        let mut bytes = [0u8; FRAME_LEN];
        bytes[0] = PACKET_HEADER;
        write_u24(&mut bytes[1..4], 251_066);
        write_u24(&mut bytes[4..7], 15_883 * 230);
        write_u24(&mut bytes[7..10], 0);
        write_u24(&mut bytes[10..13], 623 * 100);
        write_u24(&mut bytes[13..16], 5_347);
        bytes[16..18].copy_from_slice(&20_000u16.to_le_bytes());
        bytes[19] = 0x42;
        bytes[22] = frame_checksum(address, &bytes[..22]);
        bytes
    }

    #[test]
    fn parses_and_scales_full_packet() {
        let raw = RawFrame::parse(&sample_frame(0), 0).unwrap();
        let reading = Measurement::from_raw(raw, Calibration::common_reference_board());
        assert_eq!(reading.voltage_mv, 230_000);
        assert_eq!(reading.current_ma, 1_000);
        assert_eq!(reading.active_power_mw, 100_000);
        assert_eq!(reading.frequency_millihz, Some(50_000));
        assert_eq!(reading.counter_energy_uwh, 1_000_000_000);
        assert_eq!(reading.status, 0x42);
    }

    #[test]
    fn signed_24_bit_power_is_preserved() {
        let mut bytes = sample_frame(0);
        write_u24(&mut bytes[10..13], 0xFF_FFFF);
        bytes[22] = frame_checksum(0, &bytes[..22]);
        assert_eq!(RawFrame::parse(&bytes, 0).unwrap().active_power, -1);
    }

    #[test]
    fn stream_parser_ignores_noise_and_emits_frame() {
        let mut parser = StreamParser::new(0).unwrap();
        assert_eq!(parser.push(0x12), FeedResult::Pending);
        let frame = sample_frame(0);
        let mut result = FeedResult::Pending;
        for byte in frame {
            result = parser.push(byte);
        }
        assert!(matches!(result, FeedResult::Frame(_)));
        assert_eq!(parser.buffered_len(), 0);
    }

    #[test]
    fn checksum_failure_is_reported() {
        let mut frame = sample_frame(0);
        frame[5] ^= 1;
        assert!(matches!(
            RawFrame::parse(&frame, 0),
            Err(ParseError::BadChecksum { .. })
        ));
    }

    #[test]
    fn write_request_contains_protocol_checksum() {
        let packet = write_register_request(0, 0x19, 0x12_34_56).unwrap();
        assert_eq!(packet, [0xA8, 0x19, 0x56, 0x34, 0x12, 0xA2]);
    }

    #[test]
    fn energy_tracker_distinguishes_wrap_from_reset() {
        let mut tracker = EnergyTracker::new(5_347).unwrap();
        assert_eq!(tracker.observe(0xFF_FFF0), CounterEvent::First);
        assert_eq!(tracker.observe(0x00_0010), CounterEvent::Wrapped(32));
        assert_eq!(
            tracker.observe(0x10_0000),
            CounterEvent::Advanced(0x0F_FFF0)
        );
        let total_before_reset = tracker.total_uwh();
        assert_eq!(tracker.observe(100), CounterEvent::Reset);
        assert_eq!(tracker.total_uwh(), total_before_reset);
    }

    #[test]
    fn energy_tracker_accumulates_and_restores_lifetime_energy() {
        let mut tracker = EnergyTracker::new(5_347).unwrap();
        assert_eq!(tracker.observe(0), CounterEvent::First);
        assert_eq!(tracker.observe(5_347), CounterEvent::Advanced(5_347));
        assert_eq!(tracker.total_uwh(), 1_000_000_000);

        tracker.restore_total_uwh(42_000_000);
        assert_eq!(tracker.observe(123), CounterEvent::First);
        assert_eq!(tracker.total_uwh(), 42_000_000);
        assert_eq!(tracker.observe(5_470), CounterEvent::Advanced(5_347));
        assert_eq!(tracker.total_uwh(), 1_042_000_000);
    }
}
