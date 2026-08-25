//! Network-status LED policy shared by every plug board.

/// Network state shown by the dedicated status LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NetworkStatus {
    /// No active steering attempt and no joined network.
    Offline = 0,
    /// Network steering/rejoin is in progress.
    Searching = 1,
    /// The device is joined and servicing Zigbee traffic.
    Joined = 2,
    /// An unrecoverable hardware or persistence error stopped the app.
    Fault = 3,
}

impl NetworkStatus {
    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Searching,
            2 => Self::Joined,
            3 => Self::Fault,
            _ => Self::Offline,
        }
    }
}

/// Resolve the logical LED state at `now_ms`.
///
/// Searching uses a 500 ms on / 500 ms off cadence. Offline is dark and
/// Joined follows the relay, matching Tuya `backlight_mode = LightWhenOn`.
/// Fault remains solid to preserve the fail-closed diagnostic indication.
pub const fn status_led_on(status: NetworkStatus, relay_on: bool, now_ms: u32) -> bool {
    match status {
        NetworkStatus::Offline => false,
        NetworkStatus::Searching => (now_ms / 500).is_multiple_of(2),
        NetworkStatus::Joined => relay_on,
        NetworkStatus::Fault => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_is_dark_and_joined_follows_relay() {
        assert!(!status_led_on(NetworkStatus::Offline, false, 0));
        assert!(!status_led_on(NetworkStatus::Offline, true, 10_000));
        assert!(!status_led_on(NetworkStatus::Joined, false, 0));
        assert!(status_led_on(NetworkStatus::Joined, true, 10_000));
    }

    #[test]
    fn searching_blinks_at_one_hertz() {
        assert!(status_led_on(NetworkStatus::Searching, false, 0));
        assert!(status_led_on(NetworkStatus::Searching, true, 499));
        assert!(!status_led_on(NetworkStatus::Searching, false, 500));
        assert!(!status_led_on(NetworkStatus::Searching, true, 999));
        assert!(status_led_on(NetworkStatus::Searching, false, 1_000));
    }
}
