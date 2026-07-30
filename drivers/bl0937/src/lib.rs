//! BL0937 pulse-domain power metering.
//!
//! Hardware-specific GPIO capture is intentionally outside this crate. Feed
//! it CF/CF1 pulse counts from a fixed measurement window and drive SEL to the
//! level returned by [`Bl0937::sel_high`].

#![no_std]

const MICRO_WH_PER_KWH: u128 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    ZeroScale,
    ZeroPulsesPerKwh,
    InvalidModeWindowCount,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleError {
    ZeroDuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementMode {
    Current,
    Voltage,
}

impl MeasurementMode {
    const fn toggled(self) -> Self {
        match self {
            Self::Current => Self::Voltage,
            Self::Voltage => Self::Current,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelPolarity {
    HighIsVoltage,
    HighIsCurrent,
}

impl SelPolarity {
    pub const fn level_for(self, mode: MeasurementMode) -> bool {
        matches!(
            (self, mode),
            (Self::HighIsVoltage, MeasurementMode::Voltage)
                | (Self::HighIsCurrent, MeasurementMode::Current)
        )
    }
}

/// Rational conversion from pulse frequency in mHz to an integer output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    numerator: u64,
    denominator: u64,
}

impl Scale {
    pub const fn new(numerator: u64, denominator: u64) -> Result<Self, ConfigError> {
        if numerator == 0 || denominator == 0 {
            return Err(ConfigError::ZeroScale);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub fn apply(self, frequency_millihz: u64) -> u64 {
        let value = u128::from(frequency_millihz) * u128::from(self.numerator)
            / u128::from(self.denominator);
        value.min(u128::from(u64::MAX)) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calibration {
    pub power_milliwatts: Scale,
    pub current_milliamps: Scale,
    pub voltage_millivolts: Scale,
    pub cf_pulses_per_kwh: u32,
}

impl Calibration {
    pub const fn new(
        power_milliwatts: Scale,
        current_milliamps: Scale,
        voltage_millivolts: Scale,
        cf_pulses_per_kwh: u32,
    ) -> Result<Self, ConfigError> {
        if cf_pulses_per_kwh == 0 {
            return Err(ConfigError::ZeroPulsesPerKwh);
        }
        Ok(Self {
            power_milliwatts,
            current_milliamps,
            voltage_millivolts,
            cf_pulses_per_kwh,
        })
    }

    /// Derive the BL0937 datasheet conversion from board component values.
    ///
    /// `voltage_divider_milli` is the divider ratio multiplied by 1000. For
    /// example, a ratio of 2351 is represented as `2_351_000`.
    pub fn from_components(
        shunt_microohms: u32,
        voltage_divider_milli: u32,
        cf_pulses_per_kwh: u32,
    ) -> Result<Self, ConfigError> {
        if shunt_microohms == 0 || voltage_divider_milli == 0 {
            return Err(ConfigError::ZeroScale);
        }

        let shunt = u64::from(shunt_microohms);
        let divider = u64::from(voltage_divider_milli);
        let power_numerator = 1_483_524u64
            .checked_mul(divider)
            .ok_or(ConfigError::ArithmeticOverflow)?;
        let power_denominator = 1_000u64
            .checked_mul(shunt)
            .and_then(|value| value.checked_mul(1_721_506))
            .ok_or(ConfigError::ArithmeticOverflow)?;
        let current_denominator = shunt
            .checked_mul(94_638)
            .ok_or(ConfigError::ArithmeticOverflow)?;
        let voltage_numerator = 1_218u64
            .checked_mul(divider)
            .ok_or(ConfigError::ArithmeticOverflow)?;

        Self::new(
            Scale::new(power_numerator, power_denominator)?,
            Scale::new(1_218_000, current_denominator)?,
            Scale::new(voltage_numerator, 1_000_000 * 15_397)?,
            cf_pulses_per_kwh,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PulseWindow {
    pub cf_pulses: u32,
    pub cf1_pulses: u32,
    pub duration_us: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measurement {
    pub power_mw: u64,
    pub current_ma: Option<u64>,
    pub voltage_mv: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowResult {
    pub measurement: Measurement,
    pub energy_delta_uwh: u64,
    pub total_energy_uwh: u64,
    pub cf1_valid: bool,
    /// New SEL output level when the driver advances to the other CF1 mode.
    pub next_sel_high: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergyCounter {
    pulses_per_kwh: u32,
    remainder: u128,
    total_uwh: u64,
}

impl EnergyCounter {
    pub const fn new(pulses_per_kwh: u32) -> Result<Self, ConfigError> {
        if pulses_per_kwh == 0 {
            return Err(ConfigError::ZeroPulsesPerKwh);
        }
        Ok(Self {
            pulses_per_kwh,
            remainder: 0,
            total_uwh: 0,
        })
    }

    pub fn add_pulses(&mut self, pulses: u32) -> u64 {
        let numerator = self.remainder + u128::from(pulses) * MICRO_WH_PER_KWH;
        let divisor = u128::from(self.pulses_per_kwh);
        let delta = numerator / divisor;
        self.remainder = numerator % divisor;
        let delta = delta.min(u128::from(u64::MAX)) as u64;
        self.total_uwh = self.total_uwh.saturating_add(delta);
        delta
    }

    pub const fn total_uwh(&self) -> u64 {
        self.total_uwh
    }

    pub fn restore_total_uwh(&mut self, total_uwh: u64) {
        self.total_uwh = total_uwh;
        self.remainder = 0;
    }
}

pub struct Bl0937 {
    calibration: Calibration,
    mode: MeasurementMode,
    sel_polarity: SelPolarity,
    windows_per_mode: u8,
    windows_since_switch: u8,
    energy: EnergyCounter,
}

impl Bl0937 {
    pub fn new(
        calibration: Calibration,
        initial_mode: MeasurementMode,
        sel_polarity: SelPolarity,
        windows_per_mode: u8,
    ) -> Result<Self, ConfigError> {
        if windows_per_mode < 2 {
            return Err(ConfigError::InvalidModeWindowCount);
        }
        Ok(Self {
            calibration,
            mode: initial_mode,
            sel_polarity,
            windows_per_mode,
            windows_since_switch: 0,
            energy: EnergyCounter::new(calibration.cf_pulses_per_kwh)?,
        })
    }

    pub const fn mode(&self) -> MeasurementMode {
        self.mode
    }

    pub const fn sel_high(&self) -> bool {
        self.sel_polarity.level_for(self.mode)
    }

    pub const fn total_energy_uwh(&self) -> u64 {
        self.energy.total_uwh()
    }

    pub fn restore_total_energy_uwh(&mut self, total_uwh: u64) {
        self.energy.restore_total_uwh(total_uwh);
    }

    pub fn process_window(&mut self, window: PulseWindow) -> Result<WindowResult, SampleError> {
        let cf_frequency = frequency_millihz(window.cf_pulses, window.duration_us)?;
        let cf1_frequency = frequency_millihz(window.cf1_pulses, window.duration_us)?;
        let cf1_valid = self.windows_since_switch != 0;
        let mut measurement = Measurement {
            power_mw: self.calibration.power_milliwatts.apply(cf_frequency),
            current_ma: None,
            voltage_mv: None,
        };

        if cf1_valid {
            match self.mode {
                MeasurementMode::Current => {
                    measurement.current_ma =
                        Some(self.calibration.current_milliamps.apply(cf1_frequency));
                }
                MeasurementMode::Voltage => {
                    measurement.voltage_mv =
                        Some(self.calibration.voltage_millivolts.apply(cf1_frequency));
                }
            }
        }

        let energy_delta_uwh = self.energy.add_pulses(window.cf_pulses);
        self.windows_since_switch = self.windows_since_switch.saturating_add(1);
        let next_sel_high = if self.windows_since_switch >= self.windows_per_mode {
            self.mode = self.mode.toggled();
            self.windows_since_switch = 0;
            Some(self.sel_high())
        } else {
            None
        };

        Ok(WindowResult {
            measurement,
            energy_delta_uwh,
            total_energy_uwh: self.energy.total_uwh(),
            cf1_valid,
            next_sel_high,
        })
    }
}

pub fn frequency_millihz(pulses: u32, duration_us: u32) -> Result<u64, SampleError> {
    if duration_us == 0 {
        return Err(SampleError::ZeroDuration);
    }
    let value = u128::from(pulses) * 1_000_000_000u128 / u128::from(duration_us);
    Ok(value.min(u128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_calibration() -> Calibration {
        Calibration::new(
            Scale::new(2, 1).unwrap(),
            Scale::new(3, 1).unwrap(),
            Scale::new(4, 1).unwrap(),
            1_000,
        )
        .unwrap()
    }

    #[test]
    fn frequency_uses_fixed_window_duration() {
        assert_eq!(frequency_millihz(5, 1_000_000), Ok(5_000));
        assert_eq!(frequency_millihz(1, 0), Err(SampleError::ZeroDuration));
    }

    #[test]
    fn first_cf1_window_after_sel_change_is_discarded() {
        let mut driver = Bl0937::new(
            simple_calibration(),
            MeasurementMode::Current,
            SelPolarity::HighIsVoltage,
            2,
        )
        .unwrap();
        assert!(!driver.sel_high());

        let first = driver
            .process_window(PulseWindow {
                cf_pulses: 1,
                cf1_pulses: 2,
                duration_us: 1_000_000,
            })
            .unwrap();
        assert!(!first.cf1_valid);
        assert_eq!(first.measurement.current_ma, None);

        let second = driver
            .process_window(PulseWindow {
                cf_pulses: 1,
                cf1_pulses: 2,
                duration_us: 1_000_000,
            })
            .unwrap();
        assert_eq!(second.measurement.power_mw, 2_000);
        assert_eq!(second.measurement.current_ma, Some(6_000));
        assert_eq!(second.next_sel_high, Some(true));
        assert_eq!(driver.mode(), MeasurementMode::Voltage);
    }

    #[test]
    fn energy_counter_preserves_fractional_pulses() {
        let mut energy = EnergyCounter::new(3_200).unwrap();
        for _ in 0..3_200 {
            energy.add_pulses(1);
        }
        assert_eq!(energy.total_uwh(), 1_000_000_000);
    }

    #[test]
    fn component_calibration_is_finite_and_nonzero() {
        let calibration = Calibration::from_components(1_000, 2_351_000, 3_200).unwrap();
        assert!(calibration.power_milliwatts.apply(1_000) > 0);
        assert!(calibration.current_milliamps.apply(1_000) > 0);
        assert!(calibration.voltage_millivolts.apply(1_000) > 0);
    }
}
