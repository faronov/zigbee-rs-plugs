//! TLSR8258 smart-plug firmware entry point.
//!
//! Exactly one product feature selects the firmware image built here (see
//! the `compile_error!` guards below): four BL0942 products share
//! [`bl0942_app`]'s router loop and board; two incompatible BL0937 products
//! share [`bl0937_app`]'s capture-based router loop through compile-time
//! board/product selection. Neither module is compiled unless its feature
//! is selected, so a given firmware image only ever links one metering
//! driver, one board crate, and one product crate.
//!
//! EXPERIMENTAL: no image built from this crate has been run on TLSR8258
//! hardware. See `README.md` in this directory and the workspace's
//! `docs/safety.md` for the manual bring-up procedure this repository
//! currently requires.

#![no_std]
#![no_main]

// --- Exactly-one-product-feature guard -------------------------------------
//
// Zero features: every module below is `#[cfg]`'d out, so `_rust_entry`
// would have nothing to call — fail at compile time instead, with a message
// naming the expected features, rather than leaving a confusing "function
// not found" error or (worse) a firmware image with no application logic.
#[cfg(not(any(
    feature = "tz3000-gjnozsaz-1m",
    feature = "tz3000-gjnozsaz-512k",
    feature = "tz3000-w0qqde0g",
    feature = "tz3000-zloso4jk",
    feature = "legacy-bl0937-pd6",
    feature = "zbeacon-ts011f-512k",
)))]
compile_error!(
    "tlsr8258-plug requires exactly one product feature: \
     tz3000-gjnozsaz-1m, tz3000-gjnozsaz-512k, tz3000-w0qqde0g, \
     tz3000-zloso4jk, legacy-bl0937-pd6, or zbeacon-ts011f-512k"
);

// Two or more features: every pairwise combination of the six is rejected
// explicitly, rather than silently building whichever `#[cfg]` branch the
// compiler picks first — a multi-feature build must never link two
// products' worth of persistence/flash-layout assumptions into one image.
macro_rules! reject_pair {
    ($a:literal, $b:literal) => {
        #[cfg(all(feature = $a, feature = $b))]
        compile_error!(concat!(
            "tlsr8258-plug requires exactly one product feature, both '",
            $a,
            "' and '",
            $b,
            "' are enabled"
        ));
    };
}
reject_pair!("tz3000-gjnozsaz-1m", "tz3000-gjnozsaz-512k");
reject_pair!("tz3000-gjnozsaz-1m", "tz3000-w0qqde0g");
reject_pair!("tz3000-gjnozsaz-1m", "tz3000-zloso4jk");
reject_pair!("tz3000-gjnozsaz-1m", "legacy-bl0937-pd6");
reject_pair!("tz3000-gjnozsaz-1m", "zbeacon-ts011f-512k");
reject_pair!("tz3000-gjnozsaz-512k", "tz3000-w0qqde0g");
reject_pair!("tz3000-gjnozsaz-512k", "tz3000-zloso4jk");
reject_pair!("tz3000-gjnozsaz-512k", "legacy-bl0937-pd6");
reject_pair!("tz3000-gjnozsaz-512k", "zbeacon-ts011f-512k");
reject_pair!("tz3000-w0qqde0g", "tz3000-zloso4jk");
reject_pair!("tz3000-w0qqde0g", "legacy-bl0937-pd6");
reject_pair!("tz3000-w0qqde0g", "zbeacon-ts011f-512k");
reject_pair!("tz3000-zloso4jk", "legacy-bl0937-pd6");
reject_pair!("tz3000-zloso4jk", "zbeacon-ts011f-512k");
reject_pair!("legacy-bl0937-pd6", "zbeacon-ts011f-512k");

#[cfg(any(feature = "legacy-bl0937-pd6", feature = "zbeacon-ts011f-512k",))]
mod bl0937_app;
#[cfg(any(feature = "legacy-bl0937-pd6", feature = "zbeacon-ts011f-512k",))]
mod bl0937_task;
#[cfg(any(
    feature = "tz3000-gjnozsaz-1m",
    feature = "tz3000-gjnozsaz-512k",
    feature = "tz3000-w0qqde0g",
    feature = "tz3000-zloso4jk",
))]
mod bl0942_app;
#[cfg(any(
    feature = "tz3000-gjnozsaz-1m",
    feature = "tz3000-gjnozsaz-512k",
    feature = "tz3000-w0qqde0g",
    feature = "tz3000-zloso4jk",
))]
mod bl0942_task;
mod local_control;
mod persistence;
mod router_support;

use tlsr8258_rt as _;

#[panic_handler]
fn panic_handler(_info: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("nop");
        }
    }
}

/// Combined IRQ vector. Radio's own handler **must** run before capture's —
/// see `tlsr8258_hal::capture`'s module docs' "ISR ordering" section — so
/// this order is not incidental.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".ram_code")]
pub extern "C" fn irq_handler() {
    tlsr8258_hal::radio::handle_irq();
    #[cfg(any(feature = "legacy-bl0937-pd6", feature = "zbeacon-ts011f-512k",))]
    tlsr8258_hal::capture::handle_irq();
    local_control::handle_timer_irq();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _rust_entry() -> ! {
    if tlsr8258_hal::clocks::init().is_err() {
        // Fail closed: no board resources (relay/LED/etc.) exist yet at
        // this point in boot, so there is nothing for a board-level
        // `fail()` helper to safely drive to a defined state. Halt via
        // the panic handler above rather than proceeding with
        // unconfigured/uncalibrated clocks, which would silently mistime
        // every later UART/radio/RTCC/relay operation.
        panic!("tlsr8258_hal::clocks::init failed");
    }
    // Timer0 backs every bounded HAL wait and the ADC-based Zbit flash
    // voltage guard. It must be running before either product app
    // constructs its MAC, ADC, or persistence stack.
    tlsr8258_hal::timer::init();

    #[cfg(any(
        feature = "tz3000-gjnozsaz-1m",
        feature = "tz3000-gjnozsaz-512k",
        feature = "tz3000-w0qqde0g",
        feature = "tz3000-zloso4jk",
    ))]
    bl0942_app::run();

    #[cfg(any(feature = "legacy-bl0937-pd6", feature = "zbeacon-ts011f-512k",))]
    bl0937_app::run();
}
