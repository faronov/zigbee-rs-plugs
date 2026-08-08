//! Legacy TLSR8258 pin map with BL0937 pulse metering.

#![no_std]

pub use zigbee_plug_hardware::LEGACY_BL0937_PD6 as PROFILE;

use tlsr8258_hal::gpio::Pin;

pub struct Bl0937Pins {
    pub cf: Pin,
    pub cf1: Pin,
    pub sel: Pin,
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
    pub aux_leds: [Pin; 2],
    pub button: Pin,
    pub metering: Bl0937Pins,
    pub flash: OnboardFlash,
    /// Exclusive ownership token for the shared MISC-channel ADC, plus the
    /// otherwise-unused GPIO pad ([`Bl0937Pins`] does not use PC5) that
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
            pb5,
            pb6,
            pb7,
            pd3,
            pd4,
            pd5,
            pd6,
            pd7,
            pc5,
            ..
        } = peripherals.pins;
        Some(Self {
            relay: pd6,
            led: pd7,
            aux_leds: [pd5, pd4],
            button: pd3,
            metering: Bl0937Pins {
                cf: pb5,
                cf1: pb6,
                sel: pb7,
            },
            flash: OnboardFlash(()),
            adc: peripherals.adc,
            flash_voltage_pin: pc5,
        })
    }

    /// Configure every output inactive. SEL starts low (current mode).
    #[cfg(target_arch = "tc32")]
    pub fn initialize_safe(&self) -> Result<(), tlsr8258_hal::gpio::GpioError> {
        use tlsr8258_hal::gpio;

        gpio::set_function_gpio(&self.relay);
        gpio::write(&self.relay, false);
        gpio::set_output_enable(&self.relay, true);

        for led in core::iter::once(&self.led).chain(self.aux_leds.iter()) {
            gpio::set_function_gpio(led);
            gpio::write(led, false);
            gpio::set_output_enable(led, true);
        }

        gpio::set_function_gpio(&self.button);
        gpio::set_output_enable(&self.button, false);
        gpio::set_input_enable(&self.button, true)?;

        for input in [&self.metering.cf, &self.metering.cf1] {
            gpio::set_function_gpio(input);
            gpio::set_output_enable(input, false);
            gpio::set_input_enable(input, true)?;
        }

        gpio::set_function_gpio(&self.metering.sel);
        gpio::write(&self.metering.sel, false);
        gpio::set_output_enable(&self.metering.sel, true);
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
    pub fn set_sel_high(&self, high: bool) {
        set_sel_high(&self.metering.sel, high);
    }

    #[cfg(target_arch = "tc32")]
    pub fn button_pressed(&self) -> bool {
        button_pressed(&self.button)
    }
}

/// Free-function equivalents of the `BoardResources` inherent methods
/// above, taking the owned [`Pin`]s directly instead of `&BoardResources`.
///
/// Firmware that has moved `BoardResources::flash`/`metering` out of a
/// `BoardResources` value (each is consumed exactly once, by
/// persistence/metering-task setup) cannot keep borrowing the whole
/// struct afterward — Rust rejects `&resources` once any field has been
/// partially moved out of it. Destructuring `BoardResources` once into
/// its individual owned fields and driving them through these free
/// functions avoids that without duplicating this crate's pin-polarity
/// knowledge back into firmware code. Each `Pin`/array is still a single,
/// uniquely-owned value — these functions do not weaken or duplicate the
/// ownership `BoardResources::take()` already established, they just let
/// it be exercised after a one-time destructure instead of only through
/// `&self` methods.
#[cfg(target_arch = "tc32")]
pub fn set_relay(relay: &Pin, on: bool) {
    tlsr8258_hal::gpio::write(relay, on);
}

#[cfg(target_arch = "tc32")]
pub fn set_led(led: &Pin, on: bool) {
    tlsr8258_hal::gpio::write(led, on);
}

#[cfg(target_arch = "tc32")]
pub fn set_sel_high(sel: &Pin, high: bool) {
    tlsr8258_hal::gpio::write(sel, high);
}

#[cfg(target_arch = "tc32")]
pub fn button_pressed(button: &Pin) -> bool {
    tlsr8258_hal::gpio::read(button)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn profile_remains_explicitly_legacy() {
        assert_eq!(PROFILE.name, "legacy-bl0937-pd6");
        assert_eq!(PROFILE.leds.len(), 3);
    }

    #[test]
    fn onboard_flash_token_is_zero_sized() {
        assert_eq!(size_of::<OnboardFlash>(), 0);
    }

    #[test]
    fn board_resources_are_single_take() {
        assert!(BoardResources::take().is_some());
        assert!(BoardResources::take().is_none());
    }
}
