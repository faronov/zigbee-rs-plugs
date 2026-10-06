//! Mapping from generic parent-router status to the stock plug indicator.

use router_app::{RouterStatus, StatusSink};
use zigbee_plug_controller::NetworkStatus;

use crate::LocalControl;

pub const fn network_status_for_router(status: RouterStatus) -> NetworkStatus {
    match status {
        RouterStatus::Starting { .. }
        | RouterStatus::Commissioning { .. }
        | RouterStatus::Rejoining { .. }
        | RouterStatus::Recommissioning { .. }
        | RouterStatus::Resetting { .. } => NetworkStatus::Searching,
        RouterStatus::Online { .. } => NetworkStatus::Joined,
        RouterStatus::Fault { .. } => NetworkStatus::Fault,
    }
}

/// Owns the fitted local-control service while adapting router status.
pub struct PlugStatus<L> {
    local: L,
}

impl<L> PlugStatus<L> {
    pub const fn new(local: L) -> Self {
        Self { local }
    }

    pub const fn local(&self) -> &L {
        &self.local
    }

    pub fn local_mut(&mut self) -> &mut L {
        &mut self.local
    }
}

impl<L: LocalControl> StatusSink for PlugStatus<L> {
    fn set(&mut self, status: RouterStatus) {
        self.local
            .set_network_status(network_status_for_router(status));
    }
}
