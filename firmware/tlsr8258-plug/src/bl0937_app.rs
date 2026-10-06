//! TLSR8258 compositions for the two BL0937-metered products.
//!
//! Product/board/capture construction stays here. The shared network,
//! relay/protection, metering, reset, and checkpoint lifecycle is owned by
//! [`plug_router_app::PlugRouterApp`].

use core::mem::MaybeUninit;

use bl0937::{Calibration, SelPolarity};
use plug_router_app::{PlugRouterApp, PlugStatus};
use router_app::{
    NoDiagnostics, NoSupervisor, ParentRouterApp, PersistentChildren, RouterParts, RouterPolicy,
};
use zigbee_mac::telink::TelinkMac;
use zigbee_nwk::DeviceType;
use zigbee_plug_controller::PlugController;
use zigbee_plug_core::{PlugSettings, ProtectionConfig};
use zigbee_plug_profile::ZigbeePlug;
use zigbee_runtime::ZigbeeDevice;
use zigbee_runtime::node::ZigbeeNode;
use zigbee_runtime::power::PowerMode;
use zigbee_runtime::profile::{ApplicationProfile, SmartPlugReporting};
use zigbee_runtime::role::Router;
use zigbee_zcl::clusters::basic::PowerSource;

use tlsr8258_hal::gpio::Pin;

use board::BoardResources;
#[cfg(feature = "legacy-bl0937-pd6")]
use tlsr8258_legacy_bl0937 as board;
#[cfg(feature = "zbeacon-ts011f-512k")]
use tlsr8258_zbeacon_ts011f_bl0937 as board;

use crate::bl0937_task::Bl0937Task;
use crate::local_control;
use crate::plug_platform::{TlsrClock, TlsrLocalControl};
use crate::router_support;

#[cfg(feature = "legacy-bl0937-pd6")]
use legacy_bl0937_pd6 as product;
#[cfg(feature = "zbeacon-ts011f-512k")]
use zbeacon_ts011f_512k as product;

const ENDPOINT: u8 = 1;
const BUTTON_DEBOUNCE_MS: u32 = 30;
static ROUTER_POLICY: RouterPolicy = RouterPolicy::DEFAULT;

#[cfg(feature = "legacy-bl0937-pd6")]
fn product_metering_config() -> (Calibration, SelPolarity) {
    let calibration = Calibration::from_components(1_000, 2_351_000, 3_200)
        .expect("legacy reference component constants are valid");
    (calibration, SelPolarity::HighIsCurrent)
}

#[cfg(feature = "zbeacon-ts011f-512k")]
const fn product_metering_config() -> (Calibration, SelPolarity) {
    (product::BL0937_CALIBRATION, product::BL0937_SEL_POLARITY)
}

#[cfg(feature = "legacy-bl0937-pd6")]
fn product_protection_config() -> ProtectionConfig {
    ProtectionConfig::default()
}

#[cfg(feature = "zbeacon-ts011f-512k")]
const fn product_protection_config() -> ProtectionConfig {
    product::PROTECTION_CONFIG
}

fn fail_before_local_control(relay: &Pin, led: &Pin) -> ! {
    board::set_relay(relay, false);
    board::set_led(led, true);
    loop {
        core::hint::spin_loop();
    }
}

fn fail() -> ! {
    local_control::enter_fault()
}

pub fn run() -> ! {
    type Device = ZigbeeDevice<TelinkMac, Router>;

    let resources = match BoardResources::take() {
        Some(resources) => resources,
        None => loop {
            core::hint::spin_loop();
        },
    };
    if resources.initialize_safe().is_err() {
        loop {
            core::hint::spin_loop();
        }
    }

    let BoardResources {
        relay,
        led,
        button,
        metering,
        flash,
        aes,
        adc,
        flash_voltage_pin,
        ..
    } = resources;

    let (mut mac, ieee_address) =
        match router_support::mac_for_product(product::PRODUCT.flash.capacity) {
            Some(pair) => pair,
            None => fail_before_local_control(&relay, &led),
        };
    if mac.install_aes_engine(aes).is_err() {
        fail_before_local_control(&relay, &led);
    }
    if !router_support::install_flash_voltage_guard(
        adc,
        flash_voltage_pin,
        product::PRODUCT.flash.capacity,
    ) {
        fail_before_local_control(&relay, &led);
    }

    let (app_nv, mut security_store, child_store) = match product::storage::open_storage(flash) {
        Ok(stores) => stores,
        Err(_) => fail_before_local_control(&relay, &led),
    };

    let plug = match ZigbeePlug::new(SmartPlugReporting::default()) {
        Ok(plug) => plug,
        Err(_) => fail_before_local_control(&relay, &led),
    };
    let mut profile = plug.into_device_profile(ENDPOINT);

    static mut DEVICE_STORAGE: MaybeUninit<Device> = MaybeUninit::uninit();
    let device: &mut Device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::Router)
        .power_mode(PowerMode::AlwaysOn)
        .manufacturer(product::PRODUCT.stock_manufacturer.unwrap_or("zigbee-rs"))
        .model(product::PRODUCT.stock_model)
        .power_source(PowerSource::MainsSinglePhase)
        .endpoint(
            profile.endpoint(),
            profile.profile_id(),
            profile.device_id(),
            |endpoint| profile.configure_endpoint(endpoint),
        )
        .build_router_into(unsafe { &mut *core::ptr::addr_of_mut!(DEVICE_STORAGE) });

    if device
        .reset_security_state_if_identity_changed(&mut security_store, ieee_address)
        .is_err()
    {
        fail_before_local_control(&relay, &led);
    }

    let node = ZigbeeNode::new(device, &mut security_store, &mut profile);
    let router = match ParentRouterApp::new(
        node,
        PersistentChildren::new(child_store),
        &ROUTER_POLICY,
        RouterParts::new(
            PlugStatus::new(TlsrLocalControl),
            NoSupervisor,
            NoDiagnostics,
        ),
    ) {
        Ok(router) => router,
        Err(_) => fail_before_local_control(&relay, &led),
    };

    let (calibration, sel_polarity) = product_metering_config();
    let meter = match Bl0937Task::new(
        metering.cf,
        metering.cf1,
        metering.sel,
        calibration,
        sel_polarity,
    ) {
        Ok(meter) => meter,
        Err(_) => fail_before_local_control(&relay, &led),
    };
    let clock = TlsrClock::new();
    let controller = PlugController::new(
        PlugSettings::default(),
        product_protection_config(),
        BUTTON_DEBOUNCE_MS,
    );
    let mut app = match PlugRouterApp::new(router, controller, meter, app_nv, clock) {
        Ok(app) => app,
        Err(_) => fail_before_local_control(&relay, &led),
    };

    let startup = app.startup_outputs();
    local_control::init(
        relay,
        led,
        button,
        board::set_relay,
        board::set_led,
        board::button_pressed,
        startup.relay_on,
    );

    if tlsr8258_rt::block_on(app.initialize()).is_err() {
        fail();
    }
    loop {
        if tlsr8258_rt::block_on(app.step()).is_err() {
            fail();
        }
    }
}
