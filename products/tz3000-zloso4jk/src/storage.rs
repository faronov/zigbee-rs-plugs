//! Product-owned TLSR8258 persistence wiring for `tz3000-zloso4jk`.
//!
//! `target_arch = "tc32"`-only: this module wires the board's onboard
//! flash into the shared, disjoint application-NV and security-journal
//! partitions from `zigbee-plug-storage`, using this product's own
//! [`crate::PRODUCT`] flash layout (`TLSR8258_1M_LAYOUT`). It has not been
//! exercised on TLSR8258 hardware from this crate.

use tlsr8258_ts011f_bl0942::OnboardFlash;
use zigbee_plug_storage::flash::{
    ApplicationNv, NvError, SecurityStore, application_nv, security_store,
};
use zigbee_plug_storage::split_onboard_flash;

use crate::PRODUCT;

/// Build this product's application-NV log and security-counter journal
/// from the board's onboard flash.
///
/// `flash` (`BoardResources::flash`) may be split only once per boot;
/// call this exactly once with the token returned by
/// `tlsr8258_ts011f_bl0942::BoardResources::take()`.
pub fn open_storage(flash: OnboardFlash) -> Result<(ApplicationNv, SecurityStore), NvError> {
    let (app_nv_token, security_token) = split_onboard_flash(flash);
    let app_nv = application_nv(app_nv_token, PRODUCT.flash)?;
    let security = security_store(security_token, PRODUCT.flash);
    Ok((app_nv, security))
}
