//! Mapping from plug measurements to standard Zigbee clusters.

#![no_std]

use zigbee_mac::MacDriver;
use zigbee_plug_core::ElectricalSample;
use zigbee_runtime::ZigbeeDevice;
use zigbee_runtime::builder::EndpointBuilder;
use zigbee_runtime::profile::{
    ApplicationClusters, DeviceProfile, ProfileComponent, ProfileError, SmartPlug,
    SmartPlugReporting,
};
use zigbee_zcl::clusters::electrical::AcScaling;
use zigbee_zcl::clusters::metering::UNIT_KWH;
use zigbee_zcl::{DeviceId, ZclStatus};

pub const HOME_AUTOMATION_PROFILE_ID: u16 = 0x0104;

pub struct ZigbeePlug {
    inner: SmartPlug,
}

impl ZigbeePlug {
    pub fn new(reporting: SmartPlugReporting) -> Result<Self, ZclStatus> {
        let mut inner = SmartPlug::new(reporting).with_metering(UNIT_KWH, 1, 1_000);
        inner.set_electrical_scaling(AcScaling {
            voltage_multiplier: 1,
            voltage_divisor: 10,
            current_multiplier: 1,
            current_divisor: 1_000,
            power_multiplier: 1,
            power_divisor: 1,
        })?;
        Ok(Self { inner })
    }

    pub fn into_device_profile(self, endpoint: u8) -> DeviceProfile<Self> {
        DeviceProfile::new(
            endpoint,
            HOME_AUTOMATION_PROFILE_ID,
            DeviceId::MAINS_POWER_OUTLET,
            self,
        )
    }

    pub fn update_sample(&mut self, sample: ElectricalSample) {
        let voltage_decivolts =
            ((u64::from(sample.voltage_mv) + 50) / 100).min(u64::from(u16::MAX)) as u16;
        let current_milliamps = sample.current_ma.min(u32::from(u16::MAX)) as u16;
        let power_watts = milliwatts_to_watts(sample.active_power_mw);

        self.inner
            .update_electrical(zigbee_runtime::profile::ElectricalReading {
                rms_voltage: voltage_decivolts,
                rms_current: current_milliamps,
                active_power_watts: power_watts,
            });
        self.inner
            .set_instantaneous_demand_watts(i32::from(power_watts));
        self.inner
            .restore_energy_delivered_wh(sample.total_energy_wh());
    }

    pub fn is_on(&self) -> bool {
        self.inner.is_on()
    }

    pub fn tick_on_off(&mut self) {
        self.inner.tick_on_off();
    }

    pub fn apply_startup_on_off(&mut self, previous_on: bool) {
        self.inner.apply_startup_on_off(previous_on);
    }

    pub const fn inner(&self) -> &SmartPlug {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut SmartPlug {
        &mut self.inner
    }
}

impl ProfileComponent for ZigbeePlug {
    fn configure_endpoint(&self, endpoint: EndpointBuilder) -> EndpointBuilder {
        self.inner.configure_endpoint(endpoint)
    }

    fn collect_clusters<'a>(
        &'a mut self,
        endpoint: u8,
        clusters: &mut ApplicationClusters<'a>,
    ) -> Result<(), ProfileError> {
        self.inner.collect_clusters(endpoint, clusters)
    }

    fn expected_report_clusters(&self) -> usize {
        self.inner.expected_report_clusters()
    }

    fn configure_default_reporting<M: MacDriver>(
        &self,
        endpoint: u8,
        device: &mut ZigbeeDevice<M>,
    ) -> Result<(), ProfileError> {
        self.inner.configure_default_reporting(endpoint, device)
    }
}

fn milliwatts_to_watts(milliwatts: i32) -> i16 {
    let rounded = if milliwatts >= 0 {
        milliwatts.saturating_add(500) / 1_000
    } else {
        milliwatts.saturating_sub(500) / 1_000
    };
    rounded.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigbee_zcl::clusters::Cluster;
    use zigbee_zcl::clusters::electrical::{
        ATTR_AC_CURRENT_DIVISOR, ATTR_ACTIVE_POWER, ATTR_RMS_CURRENT, ATTR_RMS_VOLTAGE,
    };
    use zigbee_zcl::data_types::ZclValue;

    #[test]
    fn sample_uses_zha_friendly_scaling() {
        let mut profile = ZigbeePlug::new(SmartPlugReporting::default()).unwrap();
        profile.update_sample(ElectricalSample {
            voltage_mv: 230_040,
            current_ma: 1_234,
            active_power_mw: 282_600,
            frequency_millihz: Some(50_000),
            total_energy_uwh: 12_345_678_000,
        });

        let electrical = profile.inner().electrical().attributes();
        assert_eq!(
            electrical.get(ATTR_RMS_VOLTAGE),
            Some(&ZclValue::U16(2_300))
        );
        assert_eq!(
            electrical.get(ATTR_RMS_CURRENT),
            Some(&ZclValue::U16(1_234))
        );
        assert_eq!(electrical.get(ATTR_ACTIVE_POWER), Some(&ZclValue::I16(283)));
        assert_eq!(
            electrical.get(ATTR_AC_CURRENT_DIVISOR),
            Some(&ZclValue::U16(1_000))
        );
        assert_eq!(profile.inner().total_energy_delivered_wh(), Some(12_345));
    }

    #[test]
    fn negative_power_rounds_away_from_zero() {
        assert_eq!(milliwatts_to_watts(-1_500), -2);
        assert_eq!(milliwatts_to_watts(1_500), 2);
    }
}
