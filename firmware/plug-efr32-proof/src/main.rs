//! BRD4181A/EFR32MG21 always-on End Device smart-plug portability proof.
//!
//! Shared OnOff, local control, reset ordering, reporting, synthetic meter,
//! protection, LED policy, and checkpoint behavior live in the reusable plug
//! crates. This file only assembles fixed EFR32 resources and capabilities.
//!
//! This image has not validated mains switching, metering, radio operation,
//! entropy, low-power modes, or any other hardware behavior.

#![no_std]
#![no_main]

mod platform;
mod time_driver;
mod vectors;

use efr32mg21_brd4181a_plug::{BoardResources, storage::StorageToken};
use embassy_executor::Spawner;
use plug_efr32_proof::{
    meter::DevelopmentFixedMeter,
    profile::PlugProfile,
    storage::{ApplicationStore, FlashStorageCell, SecurityStore},
};
use plug_router_app::{AlwaysOnEndDevicePlugApp, PlugStatus};
use router_app::{AlwaysOnEndDeviceApp, NoDiagnostics, RouterParts};
use static_cell::StaticCell;
use zigbee_mac::{
    MacDriver,
    efr32s2::Efr32s2Mac,
    pib::{PibAttribute, PibValue},
};
use zigbee_nwk::DeviceType;
use zigbee_plug_controller::PlugController;
use zigbee_plug_core::{PlugSettings, ProtectionConfig};
use zigbee_runtime::{
    ZigbeeDevice, node::ZigbeeNode, power::PowerMode, profile::ApplicationProfile, role::EndDevice,
};
use zigbee_types::ChannelMask;
use zigbee_zcl::clusters::basic::PowerSource;

#[allow(unused_imports)]
use vectors::__INTERRUPTS;

type ProofDevice = ZigbeeDevice<Efr32s2Mac, EndDevice>;
type ProofEndDevice = AlwaysOnEndDeviceApp<
    'static,
    Efr32s2Mac,
    SecurityStore,
    PlugProfile,
    PlugStatus<platform::EfrLocalControl>,
    platform::EfrSupervisor,
    NoDiagnostics,
>;
type SharedPlugApp = AlwaysOnEndDevicePlugApp<
    'static,
    Efr32s2Mac,
    SecurityStore,
    platform::EfrLocalControl,
    platform::EfrSupervisor,
    NoDiagnostics,
    DevelopmentFixedMeter,
    ApplicationStore,
    platform::EfrPlugClock,
>;

const _: () = assert!(!plug_efr32_proof::CHILD_ADMISSION);

/// Post-link proof markers retained by an explicit startup reference.
#[used]
#[unsafe(no_mangle)]
pub static EFR32_ALWAYS_ON_END_DEVICE: u32 = 0x454E_4444;
#[used]
#[unsafe(no_mangle)]
pub static EFR32S2_MAC_SOFTWARE_AES_KAT: u32 = 0x4145_534B;

static FLASH_CELL: FlashStorageCell = FlashStorageCell::new();
static SECURITY: StaticCell<SecurityStore> = StaticCell::new();
static PROFILE: StaticCell<PlugProfile> = StaticCell::new();
static DEVICE: StaticCell<ProofDevice> = StaticCell::new();
static APP: StaticCell<SharedPlugApp> = StaticCell::new();

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    platform::enter_fault()
}

fn retain_proof_markers() {
    core::hint::black_box(&EFR32_ALWAYS_ON_END_DEVICE);
    core::hint::black_box(&EFR32S2_MAC_SOFTWARE_AES_KAT);
}

fn open_storage(token: StorageToken) -> (ApplicationStore, &'static mut SecurityStore) {
    let (application, security) = plug_efr32_proof::storage::open_storage(token, &FLASH_CELL)
        .unwrap_or_else(|_| platform::reset());
    (application, SECURITY.init(security))
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    retain_proof_markers();

    let BoardResources {
        clocks,
        led,
        button,
        relay,
        storage,
    } = BoardResources::take().unwrap_or_else(|| platform::reset());

    efr32mg21_brd4181a_plug::init_clocks(clocks).unwrap_or_else(|_| platform::reset());
    time_driver::init();
    rtt_target::rtt_init_log!(log::LevelFilter::Info);

    log::info!(
        "EFR32 plug proof: {} / {} / {}",
        efr32mg21_brd4181a_plug::BOARD_RADIO,
        efr32mg21_brd4181a_plug::BOARD_MAIN,
        efr32mg21_brd4181a_plug::MCU_PART
    );
    log::warn!("relay PC3/EXP10 is a low-voltage proof output only");
    log::warn!("meter is synthetic fixed zero-load development data");
    log::warn!("radio, entropy, AES hardware, sleep, and hardware operation are unvalidated");

    let (application_store, security_store) = open_storage(storage);

    let mut mac = Efr32s2Mac::new();
    if !plug_efr32_proof::crypto::aes_startup_known_answer_test(&mut mac) {
        platform::reset();
    }
    log::info!(
        "AES startup KAT passed using {}",
        plug_efr32_proof::crypto::AES_IMPLEMENTATION
    );

    cortex_m::peripheral::NVIC::unpend(vectors::Interrupt::FrcPri);
    // SAFETY: Efr32s2Mac is the sole owner of FRC_PRI in this composition.
    unsafe { cortex_m::peripheral::NVIC::unmask(vectors::Interrupt::FrcPri) };

    let ieee_address = match mac.mlme_get(PibAttribute::MacExtendedAddress).await {
        Ok(PibValue::ExtendedAddress(address)) if address != [0; 8] && address != [0xFF; 8] => {
            address
        }
        _ => platform::reset(),
    };

    let profile = PROFILE.init(plug_efr32_proof::profile::plug_profile());
    let device: &'static mut ProofDevice = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .power_mode(PowerMode::AlwaysOn)
        .manufacturer(plug_efr32_proof::MANUFACTURER)
        .model(plug_efr32_proof::MODEL)
        .application_version(plug_efr32_proof::APPLICATION_VERSION)
        .date_code(plug_efr32_proof::DATE_CODE)
        .sw_build(plug_efr32_proof::SW_BUILD)
        .power_source(PowerSource::MainsSinglePhase)
        .channels(ChannelMask::ALL_2_4GHZ)
        .endpoint(
            profile.endpoint(),
            profile.profile_id(),
            profile.device_id(),
            |endpoint| profile.configure_endpoint(endpoint),
        )
        .build_into(DEVICE.uninit());

    // Durable keys and counters must never resume under a different EUI-64.
    if device
        .reset_security_state_if_identity_changed(security_store, ieee_address)
        .is_err()
    {
        platform::reset();
    }

    let node = ZigbeeNode::new(device, security_store, profile);
    let end_device: ProofEndDevice = AlwaysOnEndDeviceApp::new(
        node,
        &plug_efr32_proof::policy::ALWAYS_ON_END_DEVICE_POLICY,
        RouterParts::new(
            PlugStatus::new(platform::EfrLocalControl),
            platform::EfrSupervisor,
            NoDiagnostics,
        ),
    )
    .unwrap_or_else(|_| platform::reset());

    let controller = PlugController::new(
        PlugSettings::default(),
        ProtectionConfig::default(),
        plug_efr32_proof::policy::BUTTON_DEBOUNCE_MS,
    );
    let app = AlwaysOnEndDevicePlugApp::new(
        end_device,
        controller,
        DevelopmentFixedMeter::new(),
        application_store,
        platform::EfrPlugClock,
    )
    .unwrap_or_else(|_| platform::reset());
    let app = APP.init(app);

    platform::init_local_control(
        relay,
        led.into_led(),
        button.into_button(),
        app.startup_outputs().relay_on,
        embassy_time::Instant::now().as_millis() as u32,
    );

    if app.initialize().await.is_err() {
        platform::enter_fault();
    }
    loop {
        if app.step().await.is_err() {
            platform::enter_fault();
        }
    }
}
