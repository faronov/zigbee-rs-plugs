#![no_std]

use tlsr8258_hal::gpio::Pin;
use zigbee_plug_hardware::{
    ActiveLevel, BoardProfile, Input, MeteringPins, Output, Pin as ProfilePin, Port,
};

const LEDS: [Output; 1] = [Output {
    pin: ProfilePin::new(Port::B, 1),
    active: ActiveLevel::Low,
}];

pub const PROFILE: BoardProfile = BoardProfile {
    name: "zbeacon-ts011f-bl0937-pd2",
    relay: Output {
        pin: ProfilePin::new(Port::D, 2),
        active: ActiveLevel::High,
    },
    leds: &LEDS,
    button: Input {
        pin: ProfilePin::new(Port::A, 0),
        active: ActiveLevel::Low,
        pull_up_ohms: Some(10_000),
    },
    metering: MeteringPins::Bl0937 {
        cf: ProfilePin::new(Port::B, 4),
        cf1: ProfilePin::new(Port::B, 5),
        sel: Output {
            pin: ProfilePin::new(Port::D, 3),
            active: ActiveLevel::High,
        },
    },
};

pub struct Bl0937Pins {
    pub cf: Pin,
    pub cf1: Pin,
    pub sel: Pin,
}

pub struct OnboardFlash(());

pub struct BoardResources {
    pub relay: Pin,
    pub led: Pin,
    pub button: Pin,
    pub metering: Bl0937Pins,
    pub flash: OnboardFlash,
    /// Exclusive hardware AES-128 accelerator token. The firmware consumes
    /// it once when installing the fail-closed hardware crypto provider into
    /// `TelinkMac`; no software fallback is available in production.
    pub aes: tlsr8258_hal::peripherals::Aes,
    pub adc: tlsr8258_hal::peripherals::Adc,
    pub flash_voltage_pin: Pin,
}

impl BoardResources {
    pub fn take() -> Option<Self> {
        let peripherals = tlsr8258_hal::peripherals::Peripherals::take()?;
        let tlsr8258_hal::peripherals::Pins {
            pa0,
            pb1,
            pb4,
            pb5,
            pc5,
            pd2,
            pd3,
            ..
        } = peripherals.pins;
        Some(Self {
            relay: pd2,
            led: pb1,
            button: pa0,
            metering: Bl0937Pins {
                cf: pb4,
                cf1: pb5,
                sel: pd3,
            },
            flash: OnboardFlash(()),
            aes: peripherals.aes,
            adc: peripherals.adc,
            flash_voltage_pin: pc5,
        })
    }

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

        for input in [&self.metering.cf, &self.metering.cf1] {
            gpio::set_function_gpio(input);
            gpio::set_output_enable(input, false);
            gpio::set_input_enable(input, true)?;
            gpio::set_pull(input, Pull::PullUp10K)?;
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
    pub fn button_pressed(&self) -> bool {
        button_pressed(&self.button)
    }
}

#[cfg(target_arch = "tc32")]
pub fn set_relay(relay: &Pin, on: bool) {
    tlsr8258_hal::gpio::write(relay, on);
}

#[cfg(target_arch = "tc32")]
pub fn set_led(led: &Pin, on: bool) {
    tlsr8258_hal::gpio::write(led, !on);
}

#[cfg(target_arch = "tc32")]
pub fn button_pressed(button: &Pin) -> bool {
    !tlsr8258_hal::gpio::read(button)
}

#[cfg(test)]
mod tests {
    use super::PROFILE;
    use core::mem::size_of;
    use zigbee_plug_hardware::{ActiveLevel, MeteringPins, Pin, Port};

    #[test]
    fn profile_matches_reverse_engineered_flash_pin_map() {
        assert_eq!(PROFILE.name, "zbeacon-ts011f-bl0937-pd2");
        assert_eq!(PROFILE.relay.pin, Pin::new(Port::D, 2));
        assert_eq!(PROFILE.relay.active, ActiveLevel::High);
        assert_eq!(PROFILE.leds[0].pin, Pin::new(Port::B, 1));
        assert_eq!(PROFILE.leds[0].active, ActiveLevel::Low);
        assert_eq!(PROFILE.button.pin, Pin::new(Port::A, 0));
        assert_eq!(PROFILE.button.active, ActiveLevel::Low);
        assert_eq!(PROFILE.button.pull_up_ohms, Some(10_000));
        assert!(matches!(
            PROFILE.metering,
            MeteringPins::Bl0937 {
                cf: Pin {
                    port: Port::B,
                    bit: 4
                },
                cf1: Pin {
                    port: Port::B,
                    bit: 5
                },
                sel: zigbee_plug_hardware::Output {
                    pin: Pin {
                        port: Port::D,
                        bit: 3
                    },
                    ..
                }
            }
        ));
    }

    #[test]
    fn onboard_flash_token_is_zero_sized() {
        assert_eq!(size_of::<super::OnboardFlash>(), 0);
        assert_eq!(size_of::<tlsr8258_hal::peripherals::Aes>(), 0);
    }

    #[test]
    fn board_resources_are_single_take() {
        assert!(super::BoardResources::take().is_some());
        assert!(super::BoardResources::take().is_none());
    }
}
