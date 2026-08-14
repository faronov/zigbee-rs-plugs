//! Shared commissioning-loop plumbing for both product families' router
//! loops (`bl0942_app.rs`/`bl0937_app.rs`).
//!
//! Kept intentionally minimal: the board types differ in pins and metering
//! peripherals, so the full board/product/task wiring stays in each app
//! module. This module only shares what generalizes over any
//! `ZigbeeNode<M, S, P, R>`, following `zigbee-rs`'s own
//! `examples/telink-tlsr8258-router` control flow except that the Basic
//! cluster's Reset to Factory Defaults event remains distinct from a Zigbee
//! network factory reset.

use tlsr8258_hal::flash::FlashGeometry;
use zigbee_mac::MacDriver;
use zigbee_mac::telink::TelinkMac;
use zigbee_runtime::event_loop::{StackEvent, StartError};
use zigbee_runtime::node::ZigbeeNode;
use zigbee_runtime::profile::ApplicationProfile;
use zigbee_runtime::role::DeviceRole;
use zigbee_runtime::security_store::SecurityStateStore;

/// Construct a [`TelinkMac`] from the product's declared flash capacity,
/// using the upstream geometry-aware factory-identity primitives
/// (`tlsr8258_hal::flash::FlashGeometry`,
/// `zigbee_mac::telink::TelinkMac::new_for_flash_geometry`).
///
/// # Why this replaced the earlier compile-time capacity gate
///
/// Previously, the only available upstream primitive
/// (`tlsr8258_hal::flash::factory_ieee`) was hardcoded to the legacy
/// TB-04 512 KiB Telink factory-sector address (`0x76000`) with no
/// geometry parameter, so this crate blocked every non-512-KiB product at
/// compile time rather than risk silently reading a "plausible but
/// incorrect" identity from the wrong sector.
///
/// The upstream HAL and `zigbee-mac` now expose geometry-aware
/// alternatives: [`FlashGeometry::from_capacity`] maps a product's byte
/// capacity to its `FlashGeometry` (`KiB512`/`MiB1`/`MiB2`/`MiB4`, each
/// with its own factory-EUI and ADC-calibration address —
/// `flash::FlashGeometry::adc_calibration_address`), and
/// [`TelinkMac::new_for_flash_geometry`] verifies the *fitted* JEDEC flash
/// against that geometry before reading its factory sector, returning
/// `Err` rather than a value if the fitted part does not actually match.
/// This finally makes runtime detection of "wrong sector" possible, so a
/// compile-time block is no longer the only safe option.
///
/// # Failure handling
///
/// Returns `None` — the caller's fail-closed path, not a fabricated
/// identity — when either:
/// - `capacity` does not correspond to any known `FlashGeometry` (product
///   metadata error: [`FlashGeometry::from_capacity`] returned `None`), or
/// - the fitted flash's JEDEC capacity does not match the product-selected
///   geometry, or any other `FlashError` from
///   [`TelinkMac::new_for_flash_geometry`] (e.g. a future flash-voltage
///   guard failure) — that error is intentionally not exposed further,
///   since every call site's only correct response is the same
///   diverging fail-closed sequence.
///
/// No per-product EUI byte offset is applied anywhere in this path: only
/// one product's firmware ever runs on a given physical part at a time,
/// so there is no real identity collision to avoid, and perturbing a
/// factory-assigned (or HAL flash-UID-derived) EUI-64 would corrupt its
/// OUI/U-L-bit structure and force a spurious new Zigbee identity on every
/// reflash for no benefit.
///
/// Also returns the resolved factory EUI-64 alongside the constructed
/// `TelinkMac`: `zigbee_runtime`'s
/// `reset_security_state_if_identity_changed` needs that address as a
/// plain value to detect a reprogrammed/replaced part, and `TelinkMac`
/// itself exposes no public getter for the address it was built with.
/// This does perform the factory-sector read twice (once directly via
/// `factory_ieee_for`, once inside `TelinkMac::new_for_flash_geometry`),
/// but both reads are deterministic and geometry-verified, so the two
/// values are always identical — the minor redundant flash read at
/// startup is preferred over re-deriving `TelinkMac` construction outside
/// its own constructor.
pub fn mac_for_product(flash_capacity: u32) -> Option<(TelinkMac, [u8; 8])> {
    let geometry = FlashGeometry::from_capacity(flash_capacity as usize)?;
    let mut ieee_address = [0u8; 8];
    tlsr8258_hal::flash::factory_ieee_for(geometry, &mut ieee_address).ok()?;
    let mac = TelinkMac::new_for_flash_geometry(geometry).ok()?;
    Some((mac, ieee_address))
}

/// Install Telink's real Zbit flash-voltage guard — an ADC-sampled,
/// output-high PC5 VBAT measurement taken before every program/erase of a
/// Zbit-branded part — using this product's declared flash capacity to
/// load the matching factory ADC calibration.
///
/// # Why this replaces a constant/fake voltage callback
///
/// `tlsr8258-hal::flash::ensure_safe_flash` already fails closed
/// (`FlashError::VoltageGuardUnavailable`) on every Zbit program/erase
/// until a real guard is installed — this repository has never wired in
/// a fabricated fixed reading (e.g. a constant 3300 mV) in its place. This
/// function installs the actual measurement path
/// (`tlsr8258_hal::adc::Adc::new` + `Adc::install_flash_voltage_guard`)
/// so that fail-closed default is replaced by a real reading rather than
/// staying permanently unavailable on hardware that does have Zbit flash
/// fitted.
///
/// # Failure handling
///
/// Returns `false` — the caller's fail-closed path — when either
/// `flash_capacity` has no known [`FlashGeometry`], `Adc::new`'s JEDEC/
/// geometry verification fails, or installing the guard itself fails
/// (e.g. an ADC hardware-initialization error). Every one of those is a
/// reason not to trust this board's storage-safety envelope, so the
/// caller must not proceed to open or write persistent storage; there is
/// no fallback that opens storage without the guard "just in case" the
/// fitted part turns out not to be Zbit-branded (the HAL's own
/// `ensure_safe_flash` already handles that non-Zbit case safely on its
/// own, without any guard being installed at all).
pub fn install_flash_voltage_guard(
    adc: tlsr8258_hal::peripherals::Adc,
    pin: tlsr8258_hal::gpio::Pin,
    flash_capacity: u32,
) -> bool {
    let geometry = match FlashGeometry::from_capacity(flash_capacity as usize) {
        Some(geometry) => geometry,
        None => return false,
    };
    match tlsr8258_hal::adc::Adc::new(adc, geometry) {
        Ok(adc) => adc.install_flash_voltage_guard(pin).is_ok(),
        Err(_) => false,
    }
}

/// What the main commissioning loop should do next after handling one
/// [`StackEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopControl {
    /// Keep running the current commissioning session.
    Continue,
    /// Restart from the outer commissioning loop (rejoin/re-steer).
    Recommission,
    /// Unrecoverable: the caller should enter its own failure state.
    Fatal,
}

/// Apply one stack event's required side effect (secure rejoin, leave/
/// factory reset, or falling back to recommissioning for every other
/// network-loss event).
///
/// Role-generic over `R` (see [`zigbee_runtime::role`]): a plug image only
/// ever instantiates this for [`zigbee_runtime::role::Router`], but keeping
/// the bound at [`DeviceRole`] matches the upstream reference loop's own
/// role-generic `apply_stack_event` and avoids baking the parent role into
/// this shared helper.
pub async fn apply_stack_event<M, S, P, R>(
    node: &mut ZigbeeNode<'_, M, S, P, R>,
    event: StackEvent,
) -> LoopControl
where
    M: MacDriver,
    S: SecurityStateStore,
    P: ApplicationProfile,
    R: DeviceRole,
{
    match event {
        StackEvent::RejoinRequested => match node.secure_rejoin().await {
            Ok(_) => LoopControl::Continue,
            Err(StartError::PersistenceFailed(_)) => LoopControl::Fatal,
            Err(_) => LoopControl::Recommission,
        },
        StackEvent::LeaveRequested => match node.factory_reset().await {
            Ok(()) => LoopControl::Recommission,
            Err(_) => LoopControl::Fatal,
        },
        StackEvent::BasicResetToFactoryDefaults => {
            // The Basic cluster already reset its writable attributes.
            // It must not erase Zigbee network credentials.
            LoopControl::Continue
        }
        StackEvent::Left | StackEvent::CommissioningComplete { success: false } => {
            LoopControl::Recommission
        }
        _ => LoopControl::Continue,
    }
}
