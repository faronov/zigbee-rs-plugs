#![no_std]

#[cfg(target_arch = "tc32")]
pub mod storage;

use bl0937::{Calibration, Scale, SelPolarity};
use zigbee_plug_core::ProtectionConfig;
use zigbee_plug_hardware::{Evidence, ProductProfile, TLSR8258_512K_LAYOUT};

const fn scale(numerator: u64, denominator: u64) -> Scale {
    match Scale::new(numerator, denominator) {
        Ok(scale) => scale,
        Err(_) => panic!("stock BL0937 scale must be nonzero"),
    }
}

const fn stock_bl0937_calibration() -> Calibration {
    // The stock record at 0x74208 contains three IEEE-754 gains followed by
    // integer output scalers:
    //
    // current: 0x41814AFD (16.1616153717), scaler 200 mA
    // voltage: 0x421C8BA3 (39.1363639832), scaler 5 V
    // power:   0x41BC41C3 (23.5321102142), scaler 36 W
    //
    // Stock conversion is frequency_hz * scaler / gain. The exact rational
    // forms below preserve the original float bit patterns without linking
    // soft-float code. Voltage and power convert V/W per Hz to mV/mW per mHz
    // directly; current needs an additional /1000 because its scaler already
    // produces mA from Hz.
    let current = scale(104_857_600, 8_473_341_000);
    let voltage = scale(1_310_720, 10_259_363);
    let power = scale(18_874_368, 12_337_603);

    // One CF pulse carries (36 / gain) joules, so the stock power gain maps
    // to 2_353_211 pulses/kWh after nearest-integer rounding.
    match Calibration::new(power, current, voltage, 2_353_211) {
        Ok(calibration) => calibration,
        Err(_) => panic!("stock BL0937 calibration must be valid"),
    }
}

pub const BL0937_CALIBRATION: Calibration = stock_bl0937_calibration();

// Static analysis of the stock SEL pipeline shows HIGH selecting voltage and
// LOW selecting current. Hardware validation remains required before treating
// metering as production-calibrated.
pub const BL0937_SEL_POLARITY: SelPolarity = SelPolarity::HighIsVoltage;

pub const PROTECTION_CONFIG: ProtectionConfig = ProtectionConfig {
    min_voltage_mv: 75_000,
    max_voltage_mv: 270_000,
    max_current_ma: 20_500,
    // The recovered stock record and trip paths contain no power threshold.
    max_power_mw: 0,
    // Thresholds are stock-derived; timing remains the conservative Rust
    // policy until the stock debounce interval is recovered.
    trip_delay_ms: 5_000,
    auto_restart_voltage: false,
    restart_delay_ms: 30_000,
};

pub const PRODUCT: ProductProfile = ProductProfile {
    slug: "zbeacon-ts011f-512k",
    stock_manufacturer: Some("Zbeacon"),
    stock_model: "TS011F",
    flash: TLSR8258_512K_LAYOUT,
    stock_ota: None,
    evidence: Evidence::PinMapOnly,
};

const _: () = assert!(PRODUCT.validate().is_ok());

#[cfg(test)]
mod tests {
    use super::{BL0937_CALIBRATION, BL0937_SEL_POLARITY, PROTECTION_CONFIG};
    use bl0937::{MeasurementMode, SelPolarity};

    #[test]
    fn stock_calibration_preserves_dump_gain_scalers() {
        assert_eq!(BL0937_CALIBRATION.current_milliamps.apply(80_808), 999);
        assert_eq!(
            BL0937_CALIBRATION.voltage_millivolts.apply(1_800_000),
            229_965
        );
        assert_eq!(
            BL0937_CALIBRATION.power_milliwatts.apply(653_670),
            1_000_000
        );
        assert_eq!(BL0937_CALIBRATION.cf_pulses_per_kwh, 2_353_211);
    }

    #[test]
    fn stock_sel_high_selects_voltage() {
        assert_eq!(BL0937_SEL_POLARITY, SelPolarity::HighIsVoltage);
        assert!(BL0937_SEL_POLARITY.level_for(MeasurementMode::Voltage));
        assert!(!BL0937_SEL_POLARITY.level_for(MeasurementMode::Current));
    }

    #[test]
    fn stock_protection_thresholds_are_product_specific() {
        assert_eq!(PROTECTION_CONFIG.min_voltage_mv, 75_000);
        assert_eq!(PROTECTION_CONFIG.max_voltage_mv, 270_000);
        assert_eq!(PROTECTION_CONFIG.max_current_ma, 20_500);
        assert_eq!(PROTECTION_CONFIG.max_power_mw, 0);
        let auto_restart_voltage = PROTECTION_CONFIG.auto_restart_voltage;
        assert!(!auto_restart_voltage);
    }
}
