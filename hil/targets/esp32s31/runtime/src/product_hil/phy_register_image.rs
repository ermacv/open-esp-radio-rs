//! Read-only windows of the production radio-PHY register image.
//!
//! The product task installs the shared radio once it owns it; each request
//! then reads a window of the published `RadioPhyPeripherals` image under
//! the radio lease, which serializes it with PHY tracking and radio work.

use embassy_sync::once_lock::OnceLock;
use oer_hil_protocol::{
    Event, PHY_REGISTER_IMAGE_WORDS, PhyRegisterImageRequest, PhyRegisterImageWords, RejectReason,
};

static RADIO: OnceLock<&'static super::SharedRadio> = OnceLock::new();

/// Make the product's shared radio observable.
pub(super) fn install(radio: &'static super::SharedRadio) {
    let _ = RADIO.init(radio);
}

/// Read the requested window, or reject a window outside the image or
/// wider than one reply.
pub(crate) async fn read(request: PhyRegisterImageRequest) -> Event {
    let Some(radio) = RADIO.try_get() else {
        return Event::Rejected(RejectReason::InvalidState);
    };
    let mut guard = radio.lock().await;
    let lease = guard.lease();
    let length = lease.phy_register_image_len();
    let first = usize::from(request.first);
    let count = usize::from(request.count);
    if count > PHY_REGISTER_IMAGE_WORDS || first + count > length {
        return Event::Rejected(RejectReason::InvalidConfiguration);
    }
    let mut values = heapless::Vec::new();
    for index in first..first + count {
        let Some(value) = lease.phy_register_image(index) else {
            return Event::Rejected(RejectReason::InvalidConfiguration);
        };
        let _ = values.push(value);
    }
    Event::PhyRegisterImage(PhyRegisterImageWords {
        first: request.first,
        length: length as u16,
        values,
    })
}
