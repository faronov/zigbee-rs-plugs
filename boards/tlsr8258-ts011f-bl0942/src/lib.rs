//! TLSR8258 TS011F board with BL0942 metering.

#![no_std]

use tlsr8258_hal::gpio::Pin;
use zigbee_plug_hardware::{
    ActiveLevel, BoardProfile, Input, MeteringPins, Output, Pin as ProfilePin, Port,
};

const LEDS: [Output; 1] = [Output {
    pin: ProfilePin::new(Port::B, 4),
    active: ActiveLevel::Low,
}];

pub const PROFILE: BoardProfile = BoardProfile {
    name: "ts011f-bl0942-pc2",
    relay: Output {
        pin: ProfilePin::new(Port::C, 2),
        active: ActiveLevel::High,
    },
    leds: &LEDS,
    button: Input {
        pin: ProfilePin::new(Port::B, 5),
        active: ActiveLevel::Low,
        pull_up_ohms: Some(10_000),
    },
    metering: MeteringPins::Bl0942 {
        uart_tx: ProfilePin::new(Port::B, 1),
        uart_rx: ProfilePin::new(Port::B, 7),
        baud: 4_800,
    },
};

pub struct Bl0942Pins {
    pub tx: Pin,
    pub rx: Pin,
}

/// Exclusive ownership token for this board's fitted onboard TLSR8258 flash.
///
/// Zero-sized. The only constructor is the private literal inside
/// [`BoardResources::take`], which itself succeeds at most once per boot
/// (it is gated by `tlsr8258_hal::peripherals::Peripherals::take`), so at
/// most one live `OnboardFlash` value can ever exist. `zigbee-plug-storage`
/// consumes it exactly once (`split_onboard_flash`) to derive the disjoint
/// application-NV and security-journal partition tokens, which is what
/// rules out a product safely constructing two overlapping raw-flash
/// accessors for this board.
pub struct OnboardFlash(());

pub struct BoardResources {
    pub relay: Pin,
    pub led: Pin,
    pub button: Pin,
    pub metering: Bl0942Pins,
    pub flash: OnboardFlash,
    /// Exclusive hardware AES-128 accelerator token. The firmware consumes
    /// it once when installing the fail-closed hardware crypto provider into
    /// `TelinkMac`; no software fallback is available in production.
    pub aes: tlsr8258_hal::peripherals::Aes,
    /// Exclusive ownership token for the independent, non-DMA UART
    /// controller wired to [`Bl0942Pins`] (PB1 TX / PB7 RX). Consumed by
    /// firmware's `tlsr8258_hal::uart::Uart::new` to build the metering
    /// UART driver; kept separate from `metering` so the pins remain
    /// available for `gpio` configuration independently of the UART
    /// controller's own lifecycle.
    pub uart: tlsr8258_hal::peripherals::Uart,
    /// Exclusive ownership token for the shared MISC-channel ADC, plus the
    /// otherwise-unused GPIO pad ([`Bl0942Pins`] does not use PC5) that
    /// Telink's Zbit flash-voltage guard drives as an output-high VBAT
    /// sense source. Neither is touched by `BoardResources::initialize_safe`
    /// — `tlsr8258_hal::adc::Adc::install_flash_voltage_guard` configures
    /// PC5 itself the first time it samples.
    pub adc: tlsr8258_hal::peripherals::Adc,
    pub flash_voltage_pin: Pin,
}

impl BoardResources {
    pub fn take() -> Option<Self> {
        let peripherals = tlsr8258_hal::peripherals::Peripherals::take()?;
        let tlsr8258_hal::peripherals::Pins {
            pb1,
            pb4,
            pb5,
            pb7,
            pc2,
            pc5,
            ..
        } = peripherals.pins;
        Some(Self {
            relay: pc2,
            led: pb4,
            button: pb5,
            metering: Bl0942Pins { tx: pb1, rx: pb7 },
            flash: OnboardFlash(()),
            aes: peripherals.aes,
            uart: peripherals.uart,
            adc: peripherals.adc,
            flash_voltage_pin: pc5,
        })
    }

    /// Configure the relay off before enabling its output driver.
    #[cfg(target_arch = "tc32")]
    pub fn initialize_safe(&self) -> Result<(), tlsr8258_hal::gpio::GpioError> {
        use tlsr8258_hal::gpio::{self, Pull};

        gpio::set_function_gpio(&self.relay);
        gpio::write(&self.relay, false);
        gpio::set_output_enable(&self.relay, true);

        gpio::set_function_gpio(&self.led);
        gpio::write(&self.led, true);
        gpio::set_output_enable(&self.led, true);

        gpio::set_function_gpio(&self.button);
        gpio::set_output_enable(&self.button, false);
        gpio::set_input_enable(&self.button, true)?;
        gpio::set_pull(&self.button, Pull::PullUp10K)?;
        Ok(())
    }

    #[cfg(target_arch = "tc32")]
    pub fn set_relay(&self, on: bool) {
        set_relay(&self.relay, on);
    }

    #[cfg(target_arch = "tc32")]
    pub fn set_led(&self, on: bool) {
        set_led(&self.led, on);
    }

    #[cfg(target_arch = "tc32")]
    pub fn button_pressed(&self) -> bool {
        button_pressed(&self.button)
    }
}

/// Free-function equivalents of [`BoardResources::set_relay`]/
/// [`BoardResources::set_led`]/[`BoardResources::button_pressed`], taking
/// the owned [`Pin`] directly instead of `&BoardResources`.
///
/// Firmware that has moved [`BoardResources::flash`]/`metering`/`uart` out
/// of a `BoardResources` value (each is consumed exactly once by
/// persistence/UART setup) cannot keep borrowing the whole struct
/// afterward — Rust rejects `&resources` once any field has been partially
/// moved out of it. Destructuring `BoardResources` once into its
/// individual owned `Pin`s and driving them through these free functions
/// avoids that without duplicating this crate's pin-polarity knowledge
/// (active-low LED, active-low button) back into firmware code. Each
/// `Pin` is still a single, uniquely-owned value — these functions do not
/// weaken or duplicate the ownership `BoardResources::take()` already
/// established, they just let it be exercised after a one-time
/// destructure instead of only through `&self` methods.
#[cfg(target_arch = "tc32")]
pub fn set_relay(relay: &tlsr8258_hal::gpio::Pin, on: bool) {
    tlsr8258_hal::gpio::write(relay, on);
}

#[cfg(target_arch = "tc32")]
pub fn set_led(led: &tlsr8258_hal::gpio::Pin, on: bool) {
    tlsr8258_hal::gpio::write(led, !on);
}

#[cfg(target_arch = "tc32")]
pub fn button_pressed(button: &tlsr8258_hal::gpio::Pin) -> bool {
    !tlsr8258_hal::gpio::read(button)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn profile_matches_bl0942_board() {
        assert_eq!(PROFILE.name, "ts011f-bl0942-pc2");
        assert_eq!(PROFILE.leds.len(), 1);
    }

    #[test]
    fn onboard_flash_token_is_zero_sized() {
        assert_eq!(size_of::<OnboardFlash>(), 0);
        assert_eq!(size_of::<tlsr8258_hal::peripherals::Aes>(), 0);
    }

    #[test]
    fn board_resources_are_single_take() {
        assert!(BoardResources::take().is_some());
        assert!(BoardResources::take().is_none());
    }
}
