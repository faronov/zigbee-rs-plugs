//! TLSR8258 composition for the four BL0942-metered products.
//!
//! Product/board/UART construction stays here. The shared network,
//! relay/protection, metering, reset, and checkpoint lifecycle is owned by
//! [`plug_router_app::PlugRouterApp`].

use core::mem::MaybeUninit;

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
use tlsr8258_hal::timer;
use tlsr8258_hal::uart::{Config, Parity, StopBits, Uart};
use tlsr8258_ts011f_bl0942::BoardResources;

use crate::bl0942_task::Bl0942Task;
use crate::local_control;
use crate::plug_platform::{TlsrClock, TlsrLocalControl};
use crate::router_support;

#[cfg(feature = "tz3000-gjnozsaz-1m")]
use tz3000_gjnozsaz_1m as product;
#[cfg(feature = "tz3000-gjnozsaz-512k")]
use tz3000_gjnozsaz_512k as product;
#[cfg(feature = "tz3000-w0qqde0g")]
use tz3000_w0qqde0g as product;
#[cfg(feature = "tz3000-zloso4jk")]
use tz3000_zloso4jk as product;

const ENDPOINT: u8 = 1;
const BUTTON_DEBOUNCE_MS: u32 = 30;
const SYSTEM_CLOCK_HZ: u32 = timer::TICKS_PER_MS * 1_000;
static ROUTER_POLICY: RouterPolicy = RouterPolicy::DEFAULT;

fn uart_config(pins: tlsr8258_ts011f_bl0942::Bl0942Pins) -> Config {
    Config {
        tx: pins.tx,
        rx: pins.rx,
        baud_rate: 4_800,
        system_clock_hz: SYSTEM_CLOCK_HZ,
        parity: Parity::None,
        stop_bits: StopBits::One,
        rts: None,
        cts: None,
    }
}

fn fail_before_local_control(relay: &Pin, led: &Pin) -> ! {
    tlsr8258_ts011f_bl0942::set_relay(relay, false);
    tlsr8258_ts011f_bl0942::set_led(led, true);
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
        metering: metering_pins,
        flash,
        aes,
        uart: uart_peripheral,
        adc,
        flash_voltage_pin,
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

    let uart = match Uart::new(uart_peripheral, uart_config(metering_pins)) {
        Ok(uart) => uart,
        Err(_) => fail_before_local_control(&relay, &led),
    };
    let mut clock = TlsrClock::new();
    let meter = Bl0942Task::new(uart, clock.millis());
    let controller = PlugController::new(
        PlugSettings::default(),
        ProtectionConfig::default(),
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
        tlsr8258_ts011f_bl0942::set_relay,
        tlsr8258_ts011f_bl0942::set_led,
        tlsr8258_ts011f_bl0942::button_pressed,
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
