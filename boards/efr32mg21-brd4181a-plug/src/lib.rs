//! BRD4181A/BRD4001A resources for the EFR32 smart-plug proof.
//!
//! This crate supports exactly `EFR32MG21A020F512IM32`:
//!
//! - built-in BTN0: PD2, active low, externally biased by BRD4001A;
//! - built-in LED0: PB0, active high;
//! - relay proof output: PC3, active high, exposed as WSTK expansion header
//!   pin 10 (`BSP_EXP_HEADER10` in Silicon Labs' BRD4181A `expconfig.h`).
//!
//! PC3 is only a low-voltage logic proof point. This crate contains no mains
//! interface assumptions. Its output latch is written low *before* GPIO mode
//! changes to push-pull, using the repaired Series-2 HAL's
//! `Pin::into_push_pull(Level::Low)` ordering.

#![no_std]

pub mod storage;

use core::sync::atomic::{AtomicBool, Ordering};

use efr32mg21_hal::{
    clock::{ClockControl, ClockError, HfxoConfig, SystemClocks},
    gpio::{Disabled, Input, Level, Pin, Port, Pull, PushPull},
    peripherals::Peripherals,
};

use storage::StorageToken;

pub const BOARD_RADIO: &str = "BRD4181A";
pub const BOARD_MAIN: &str = "BRD4001A";
pub const MCU_PART: &str = "EFR32MG21A020F512IM32";
pub const HCLK_HZ: u32 = 38_400_000;
pub const HFXO_CTUNE: u16 = 133;

pub const LED_PORT: Port = Port::B;
pub const LED_PIN: u8 = 0;
pub const BUTTON_PORT: Port = Port::D;
pub const BUTTON_PIN: u8 = 2;
pub const RELAY_PORT: Port = Port::C;
pub const RELAY_PIN: u8 = 3;
pub const RELAY_EXPANSION_HEADER_PIN: u8 = 10;
pub const RELAY_ACTIVE_LEVEL: Level = Level::High;
pub const RELAY_INACTIVE_LEVEL: Level = Level::Low;

const _: () = assert!(LED_PIN < 16);
const _: () = assert!(BUTTON_PIN < 16);
const _: () = assert!(RELAY_PIN < 16);
const _: () = assert!(!(matches!(LED_PORT, Port::C) && LED_PIN == RELAY_PIN));
const _: () = assert!(!(matches!(BUTTON_PORT, Port::C) && BUTTON_PIN == RELAY_PIN));

static TAKEN: AtomicBool = AtomicBool::new(false);

/// Complete, single-owner BRD4181A resource set.
pub struct BoardResources {
    pub clocks: ClockControl,
    pub led: LedToken,
    pub button: ButtonToken,
    /// PC3 is already configured push-pull and inactive when returned.
    pub relay: RelayOutput,
    pub storage: StorageToken,
}

impl BoardResources {
    pub fn take() -> Option<Self> {
        if TAKEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }

        let mut peripherals = match Peripherals::take() {
            Some(peripherals) => peripherals,
            None => {
                TAKEN.store(false, Ordering::Release);
                return None;
            }
        };

        // SAFETY: this board singleton is the sole GPIO allocator and the
        // three selected pins are distinct in the fixed BRD4181A pin map.
        let led = unsafe { peripherals.gpio.claim_pin(LED_PORT, LED_PIN) }.ok()?;
        // SAFETY: same singleton and fixed-map argument.
        let button = unsafe { peripherals.gpio.claim_pin(BUTTON_PORT, BUTTON_PIN) }.ok()?;
        // SAFETY: same singleton and fixed-map argument.
        let relay = unsafe { peripherals.gpio.claim_pin(RELAY_PORT, RELAY_PIN) }.ok()?;

        // The HAL writes DOUTCLR before it writes the pin's MODE field. This
        // is the required fail-inactive ordering, not a later corrective write.
        let relay = RelayOutput {
            pin: relay.into_push_pull(RELAY_INACTIVE_LEVEL),
        };

        Some(Self {
            clocks: peripherals.clocks,
            led: LedToken(led),
            button: ButtonToken(button),
            relay,
            storage: StorageToken::new(peripherals.flash),
        })
    }
}

pub fn init_clocks(mut clocks: ClockControl) -> Result<SystemClocks, ClockError> {
    clocks.configure_hfxo(HfxoConfig {
        frequency_hz: HCLK_HZ,
        ctune: HFXO_CTUNE,
    })
}

/// Exclusive built-in PB0 LED ownership.
pub struct LedToken(Pin<Disabled>);

impl LedToken {
    pub fn into_led(self) -> StatusLed {
        StatusLed {
            pin: self.0.into_push_pull(Level::Low),
        }
    }
}

/// Built-in PB0 active-high status LED.
pub struct StatusLed {
    pin: Pin<PushPull>,
}

impl StatusLed {
    pub fn set(&mut self, on: bool) {
        if on {
            self.pin.set_high();
        } else {
            self.pin.set_low();
        }
    }

    pub fn is_on(&self) -> bool {
        self.pin.is_set_high()
    }
}

/// Exclusive built-in PD2 button ownership.
pub struct ButtonToken(Pin<Disabled>);

impl ButtonToken {
    /// BRD4001A supplies the button bias, so no internal pull is selected.
    pub fn into_button(self) -> UserButton {
        UserButton {
            pin: self.0.into_input(Pull::None),
        }
    }
}

/// Built-in PD2 active-low user button.
pub struct UserButton {
    pin: Pin<Input>,
}

impl UserButton {
    pub fn is_pressed(&self) -> bool {
        !self.pin.is_high()
    }
}

/// PC3/EXP10 active-high low-voltage relay proof output.
pub struct RelayOutput {
    pin: Pin<PushPull>,
}

impl RelayOutput {
    pub fn set_energized(&mut self, energized: bool) {
        if energized {
            self.pin.set_high();
        } else {
            self.pin.set_low();
        }
    }

    pub fn is_energized(&self) -> bool {
        self.pin.is_set_high()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_pin_map_matches_brd4181a_and_exp10() {
        assert_eq!((LED_PORT, LED_PIN), (Port::B, 0));
        assert_eq!((BUTTON_PORT, BUTTON_PIN), (Port::D, 2));
        assert_eq!((RELAY_PORT, RELAY_PIN), (Port::C, 3));
        assert_eq!(RELAY_EXPANSION_HEADER_PIN, 10);
        assert_eq!(RELAY_ACTIVE_LEVEL, Level::High);
        assert_eq!(RELAY_INACTIVE_LEVEL, Level::Low);
    }

    #[test]
    fn fitted_resources_do_not_alias() {
        assert_ne!((LED_PORT as u8, LED_PIN), (BUTTON_PORT as u8, BUTTON_PIN));
        assert_ne!((LED_PORT as u8, LED_PIN), (RELAY_PORT as u8, RELAY_PIN));
        assert_ne!(
            (BUTTON_PORT as u8, BUTTON_PIN),
            (RELAY_PORT as u8, RELAY_PIN)
        );
    }

    #[test]
    fn source_keeps_latch_before_output_enable() {
        let source = include_str!("lib.rs");
        let latch_first = source
            .find("relay.into_push_pull(RELAY_INACTIVE_LEVEL)")
            .expect("fail-inactive conversion");
        let resource_return = source.find("Some(Self {").expect("resource publication");
        assert!(latch_first < resource_return);
    }
}
