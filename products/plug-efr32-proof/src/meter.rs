//! Development-only fixed meter used until BRD4181A IADC wiring is proven.

use plug_router_app::{MeterService, MeterServiceOutcome};
use zigbee_plug_core::ElectricalSample;

pub const SAMPLE_INTERVAL_MS: u32 = 5_000;
pub const DEVELOPMENT_ONLY: bool = true;

/// Synthetic zero-load readings; never substitute these for calibrated data.
pub struct DevelopmentFixedMeter {
    total_energy_uwh: u64,
    last_sample_ms: u32,
    emitted: bool,
}

impl DevelopmentFixedMeter {
    pub const fn new() -> Self {
        Self {
            total_energy_uwh: 0,
            last_sample_ms: 0,
            emitted: false,
        }
    }

    const fn sample(&self) -> ElectricalSample {
        ElectricalSample {
            voltage_mv: 230_000,
            current_ma: 0,
            active_power_mw: 0,
            frequency_millihz: Some(50_000),
            total_energy_uwh: self.total_energy_uwh,
        }
    }
}

impl Default for DevelopmentFixedMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl MeterService for DevelopmentFixedMeter {
    fn restore_energy_uwh(&mut self, total_uwh: u64) {
        self.total_energy_uwh = total_uwh;
    }

    fn total_energy_uwh(&self) -> u64 {
        self.total_energy_uwh
    }

    fn service(&mut self, now_ms: u32) -> MeterServiceOutcome {
        if self.emitted && now_ms.wrapping_sub(self.last_sample_ms) < SAMPLE_INTERVAL_MS {
            return MeterServiceOutcome::Idle;
        }
        self.emitted = true;
        self.last_sample_ms = now_ms;
        MeterServiceOutcome::Sample(self.sample())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_meter_is_labeled_and_never_invents_energy() {
        assert!(core::hint::black_box(DEVELOPMENT_ONLY));
        let mut meter = DevelopmentFixedMeter::new();
        meter.restore_energy_uwh(123_456);
        let MeterServiceOutcome::Sample(sample) = meter.service(0) else {
            panic!("first fixed sample");
        };
        assert_eq!(sample.total_energy_uwh, 123_456);
        assert_eq!(sample.current_ma, 0);
        assert_eq!(sample.active_power_mw, 0);
        assert_eq!(meter.service(1), MeterServiceOutcome::Idle);
    }
}
