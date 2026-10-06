//! Cross-platform smart-plug lifecycle over statically selected network roles.
//!
//! This crate owns the product-family behavior shared by the BL0942 and
//! BL0937 firmware compositions: local relay reconciliation, protection
//! vetoes, meter servicing, button commissioning requests, factory-reset
//! ordering, the 100 ms On/Off tick, and wear-bounded application-state
//! checkpoints. Board GPIO, Timer1, meter
//! acquisition, flash construction, and product identity remain in the
//! composition root.
//!
//! With the `router` feature, `PlugRouterApp` retains the child-capable parent
//! frontend used by the Telink products. [`AlwaysOnEndDevicePlugApp`] selects a conformant
//! receiver-on End Device for a MAC that cannot implement `ParentMacDriver`;
//! both wrappers own the same private plug behavior core.

#![no_std]

mod app;
mod capabilities;
mod meter;
mod persistence;
mod status;

#[cfg(feature = "router")]
pub use app::PlugRouterApp;
pub use app::{
    AlwaysOnEndDevicePlugApp, CommissioningRequestOutcome, PlugRouterError, PlugStepOutcome,
};
pub use capabilities::{LocalControl, LocalRelaySelection, PlugClock, RelayCommand};
pub use meter::{
    MeterFault, MeterHealthPolicy, MeterSafetyState, MeterService, MeterServiceOutcome,
};
pub use persistence::{
    APP_STATE_CHECKPOINT_INTERVAL_MS, AppStateCheckpoint, AppStateRestore, CheckpointOutcome,
};
pub use status::{PlugStatus, network_status_for_router};
