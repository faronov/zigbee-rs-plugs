//! Debounced push-button edge detection.
//!
//! Deliberately tiny and hardware-independent: firmware samples the raw
//! GPIO level on every controller tick and feeds it to [`ButtonDebouncer`],
//! which only reports a [`ButtonEdge`] once the level has been stable for
//! at least the configured debounce window. This rejects relay/mains
//! contact bounce and short mechanical-switch noise without needing GPIO
//! edge interrupts.

/// A debounced press or release transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEdge {
    Pressed,
    Released,
}

/// A complete user gesture after debounce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonGestureEvent {
    /// The button was released before the long-press threshold.
    ShortPress,
    /// The button remained pressed for the configured hold time.
    LongPress,
}

/// Debounces a raw, actively-driven boolean button level.
///
/// The caller is responsible for translating its board's electrical active
/// level (see `zigbee_plug_hardware::ActiveLevel`) into the logical
/// `pressed` boolean passed to [`Self::sample`]; this type only reasons
/// about logical press/release state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonDebouncer {
    debounce_ms: u32,
    stable_pressed: bool,
    candidate_pressed: bool,
    candidate_since_ms: u32,
}

impl ButtonDebouncer {
    pub const fn new(debounce_ms: u32) -> Self {
        Self {
            debounce_ms,
            stable_pressed: false,
            candidate_pressed: false,
            candidate_since_ms: 0,
        }
    }

    /// Feed one raw sample taken at `now_ms`. Returns `Some(edge)` at most
    /// once per genuine, sustained level change.
    pub fn sample(&mut self, now_ms: u32, pressed: bool) -> Option<ButtonEdge> {
        if pressed != self.candidate_pressed {
            self.candidate_pressed = pressed;
            self.candidate_since_ms = now_ms;
            return None;
        }

        if pressed == self.stable_pressed {
            return None;
        }

        if now_ms.wrapping_sub(self.candidate_since_ms) < self.debounce_ms {
            return None;
        }

        self.stable_pressed = pressed;
        Some(if pressed {
            ButtonEdge::Pressed
        } else {
            ButtonEdge::Released
        })
    }

    pub const fn is_pressed(&self) -> bool {
        self.stable_pressed
    }
}

/// Converts debounced edges into short-press and one-shot long-press events.
///
/// A short press is emitted on release, so a four-second factory-reset hold
/// never toggles the relay first. The long-press event fires once while the
/// button is still held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonGesture {
    debouncer: ButtonDebouncer,
    long_press_ms: u32,
    pressed_since_ms: Option<u32>,
    long_press_reported: bool,
}

impl ButtonGesture {
    pub const fn new(debounce_ms: u32, long_press_ms: u32) -> Self {
        Self {
            debouncer: ButtonDebouncer::new(debounce_ms),
            long_press_ms,
            pressed_since_ms: None,
            long_press_reported: false,
        }
    }

    pub fn sample(&mut self, now_ms: u32, pressed: bool) -> Option<ButtonGestureEvent> {
        match self.debouncer.sample(now_ms, pressed) {
            Some(ButtonEdge::Pressed) => {
                self.pressed_since_ms = Some(now_ms);
                self.long_press_reported = false;
            }
            Some(ButtonEdge::Released) => {
                let was_pressed = self.pressed_since_ms.take().is_some();
                let short_press = was_pressed && !self.long_press_reported;
                self.long_press_reported = false;
                if short_press {
                    return Some(ButtonGestureEvent::ShortPress);
                }
            }
            None => {}
        }

        if self.debouncer.is_pressed()
            && !self.long_press_reported
            && self
                .pressed_since_ms
                .is_some_and(|started| now_ms.wrapping_sub(started) >= self.long_press_ms)
        {
            self.long_press_reported = true;
            return Some(ButtonGestureEvent::LongPress);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_bounce_does_not_produce_an_edge() {
        let mut debouncer = ButtonDebouncer::new(30);
        assert_eq!(debouncer.sample(0, true), None);
        // Bounces back to released before the debounce window elapses.
        assert_eq!(debouncer.sample(10, false), None);
        assert_eq!(debouncer.sample(20, true), None);
        assert!(!debouncer.is_pressed());
    }

    #[test]
    fn sustained_press_and_release_produce_exactly_one_edge_each() {
        let mut debouncer = ButtonDebouncer::new(30);
        assert_eq!(debouncer.sample(0, true), None);
        assert_eq!(debouncer.sample(29, true), None);
        assert_eq!(debouncer.sample(30, true), Some(ButtonEdge::Pressed));
        // Holding the button down must not re-fire the edge.
        assert_eq!(debouncer.sample(1_000, true), None);
        assert!(debouncer.is_pressed());

        assert_eq!(debouncer.sample(1_000, false), None);
        assert_eq!(debouncer.sample(1_030, false), Some(ButtonEdge::Released));
        assert!(!debouncer.is_pressed());
    }

    #[test]
    fn timestamp_wraparound_is_handled() {
        let mut debouncer = ButtonDebouncer::new(30);
        let near_wrap = u32::MAX - 10;
        assert_eq!(debouncer.sample(near_wrap, true), None);
        assert_eq!(
            debouncer.sample(near_wrap.wrapping_add(30), true),
            Some(ButtonEdge::Pressed)
        );
    }

    #[test]
    fn short_press_is_reported_on_release() {
        let mut gesture = ButtonGesture::new(30, 4_000);
        assert_eq!(gesture.sample(0, true), None);
        assert_eq!(gesture.sample(30, true), None);
        assert_eq!(gesture.sample(200, false), None);
        assert_eq!(
            gesture.sample(230, false),
            Some(ButtonGestureEvent::ShortPress)
        );
    }

    #[test]
    fn four_second_hold_reports_only_long_press() {
        let mut gesture = ButtonGesture::new(30, 4_000);
        assert_eq!(gesture.sample(0, true), None);
        assert_eq!(gesture.sample(30, true), None);
        assert_eq!(gesture.sample(4_029, true), None);
        assert_eq!(
            gesture.sample(4_030, true),
            Some(ButtonGestureEvent::LongPress)
        );
        assert_eq!(gesture.sample(8_000, true), None);
        assert_eq!(gesture.sample(8_010, false), None);
        assert_eq!(gesture.sample(8_040, false), None);
    }

    #[test]
    fn gesture_hold_time_is_wrap_safe() {
        let mut gesture = ButtonGesture::new(30, 4_000);
        let start = u32::MAX - 50;
        assert_eq!(gesture.sample(start, true), None);
        assert_eq!(gesture.sample(start.wrapping_add(30), true), None);
        assert_eq!(
            gesture.sample(start.wrapping_add(4_030), true),
            Some(ButtonGestureEvent::LongPress)
        );
    }
}
