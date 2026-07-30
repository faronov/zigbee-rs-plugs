//! Legacy TLSR8258 pin map with BL0937 pulse metering.

#![no_std]

pub use zigbee_plug_hardware::LEGACY_BL0937_PD6 as PROFILE;

use tlsr8258_hal::gpio::Pin;

pub struct Bl0937Pins {
    pub cf: Pin,
    pub cf1: Pin,
    pub sel: Pin,
}

pub struct BoardResources {
    pub relay: Pin,
    pub leds: [Pin; 3],
    pub button: Pin,
    pub metering: Bl0937Pins,
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
            ..
        } = peripherals.pins;
        Some(Self {
            relay: pd6,
            leds: [pd7, pd5, pd4],
            button: pd3,
            metering: Bl0937Pins {
                cf: pb5,
                cf1: pb6,
                sel: pb7,
            },
        })
    }

    /// Configure every output inactive. SEL starts low (current mode).
    #[cfg(target_arch = "tc32")]
    pub fn initialize_safe(&self) -> Result<(), tlsr8258_hal::gpio::GpioError> {
        use tlsr8258_hal::gpio;

        gpio::set_function_gpio(&self.relay);
        gpio::write(&self.relay, false);
        gpio::set_output_enable(&self.relay, true);

        for led in &self.leds {
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
        tlsr8258_hal::gpio::write(&self.relay, on);
    }

    #[cfg(target_arch = "tc32")]
    pub fn set_led(&self, index: usize, on: bool) {
        if let Some(led) = self.leds.get(index) {
            tlsr8258_hal::gpio::write(led, on);
        }
    }

    #[cfg(target_arch = "tc32")]
    pub fn set_sel_high(&self, high: bool) {
        tlsr8258_hal::gpio::write(&self.metering.sel, high);
    }

    #[cfg(target_arch = "tc32")]
    pub fn button_pressed(&self) -> bool {
        tlsr8258_hal::gpio::read(&self.button)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_remains_explicitly_legacy() {
        assert_eq!(PROFILE.name, "legacy-bl0937-pd6");
        assert_eq!(PROFILE.leds.len(), 3);
    }
}
