//! Bounded always-on scheduling and local-control timing.

use router_app::RouterPolicy;

pub const BUTTON_DEBOUNCE_MS: u32 = 30;
pub const FACTORY_RESET_HOLD_MS: u32 = 4_000;

pub static ALWAYS_ON_END_DEVICE_POLICY: RouterPolicy = RouterPolicy {
    max_receive_slice_us: 20_000,
    join_retry_initial_ms: 15_000,
    join_retry_max_ms: 60_000,
    secure_rejoin_failure_limit: 3,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_device_policy_and_stock_local_timings_are_bounded() {
        assert!(ALWAYS_ON_END_DEVICE_POLICY.is_valid());
        assert_eq!(ALWAYS_ON_END_DEVICE_POLICY.max_receive_slice_us, 20_000);
        assert_eq!(BUTTON_DEBOUNCE_MS, 30);
        assert_eq!(FACTORY_RESET_HOLD_MS, 4_000);
    }
}
