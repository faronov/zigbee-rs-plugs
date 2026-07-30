use bl0942::{Calibration, EnergyTracker, Measurement, RawFrame};
use zigbee_plug_core::{ElectricalSample, ProtectionConfig, ProtectionEngine};
use zigbee_plug_profile::ZigbeePlug;
use zigbee_runtime::profile::SmartPlugReporting;

fn main() {
    let raw = RawFrame {
        current_rms: 251_066,
        voltage_rms: 15_883 * 230,
        current_fast_rms: 0,
        active_power: 623 * 100,
        energy_counter: 5_347,
        frequency_period: 20_000,
        status: 0,
    };
    let calibration = Calibration::common_reference_board();
    let reading = Measurement::from_raw(raw, calibration);
    let mut energy =
        EnergyTracker::new(calibration.energy_counts_per_kwh).expect("reference is non-zero");
    energy.observe(0);
    energy.observe(raw.energy_counter);
    let sample = ElectricalSample {
        voltage_mv: reading.voltage_mv,
        current_ma: reading.current_ma,
        active_power_mw: reading.active_power_mw,
        frequency_millihz: reading.frequency_millihz,
        total_energy_uwh: energy.total_uwh(),
    };

    let mut profile =
        ZigbeePlug::new(SmartPlugReporting::default()).expect("fixed scaling is valid");
    profile.update_sample(sample);

    let mut protection = ProtectionEngine::new(ProtectionConfig::default());
    let _ = protection.update(0, sample, profile.is_on());

    assert_eq!(profile.inner().total_energy_delivered_wh(), Some(1_000));
}
