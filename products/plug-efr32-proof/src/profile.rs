//! Standard smart-plug endpoint selected unchanged for the EFR32 proof.

use crate::ENDPOINT;
use zigbee_plug_profile::ZigbeePlug;
use zigbee_runtime::profile::{DeviceProfile, SmartPlugReporting};

pub type PlugProfile = DeviceProfile<ZigbeePlug>;

pub fn plug_profile() -> PlugProfile {
    ZigbeePlug::new(SmartPlugReporting::default())
        .expect("the static smart-plug scaling is valid")
        .into_device_profile(ENDPOINT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigbee_mac::mock::MockMac;
    use zigbee_nwk::DeviceType;
    use zigbee_runtime::{
        ZigbeeDevice, power::PowerMode, profile::ApplicationProfile, role::EndDevice,
    };
    use zigbee_zcl::{ClusterId, DeviceId};

    #[test]
    fn profile_keeps_onoff_metering_and_electrical_clusters() {
        let profile = plug_profile();
        let device: ZigbeeDevice<_, EndDevice> = ZigbeeDevice::builder(MockMac::new([0x22; 8]))
            .device_type(DeviceType::EndDevice)
            .power_mode(PowerMode::AlwaysOn)
            .endpoint(
                profile.endpoint(),
                profile.profile_id(),
                profile.device_id(),
                |endpoint| profile.configure_endpoint(endpoint),
            )
            .build();
        let descriptor = device.bdb().zdo().find_endpoint(ENDPOINT).unwrap();

        assert_eq!(profile.device_id(), DeviceId::MAINS_POWER_OUTLET);
        for cluster in [
            ClusterId::ON_OFF.0,
            ClusterId::ELECTRICAL_MEASUREMENT.0,
            ClusterId::METERING.0,
        ] {
            assert!(descriptor.input_clusters.contains(&cluster));
        }
        assert_eq!(profile.expected_report_clusters(), 3);
    }

    #[test]
    fn role_is_receiver_on_end_device_without_child_capability() {
        assert!(!core::hint::black_box(crate::CHILD_ADMISSION));

        let profile = plug_profile();
        let device: ZigbeeDevice<_, EndDevice> = ZigbeeDevice::builder(MockMac::new([0x33; 8]))
            .device_type(DeviceType::EndDevice)
            .power_mode(PowerMode::AlwaysOn)
            .endpoint(
                profile.endpoint(),
                profile.profile_id(),
                profile.device_id(),
                |endpoint| profile.configure_endpoint(endpoint),
            )
            .build();
        assert_eq!(device.device_type(), DeviceType::EndDevice);
    }
}
