//! Product-owned TLSR8258 persistence wiring for `zbeacon-ts011f-512k`.

use tlsr8258_zbeacon_ts011f_bl0937::OnboardFlash;
use zigbee_plug_storage::flash::{
    ApplicationNv, NvError, SecurityStore, application_nv, security_store,
};
use zigbee_plug_storage::split_onboard_flash;

use crate::PRODUCT;

pub fn open_storage(flash: OnboardFlash) -> Result<(ApplicationNv, SecurityStore), NvError> {
    let (app_nv_token, security_token) = split_onboard_flash(flash);
    let app_nv = application_nv(app_nv_token, PRODUCT.flash)?;
    let security = security_store(security_token, PRODUCT.flash);
    Ok((app_nv, security))
}
