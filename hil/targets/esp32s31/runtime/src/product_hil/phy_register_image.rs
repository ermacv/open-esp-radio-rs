//! Windows of the production radio-PHY register image and analog image.
//!
//! Every open-radio HIL image places its one shared radio through [`adopt`],
//! so no image can own a radio this reader cannot see. Each request then
//! reads a window of the published `RadioPhyPeripherals` image or of the
//! analog image under the radio lease, which serializes it with PHY tracking
//! and radio work.

use embassy_sync::once_lock::OnceLock;
use oer_hil_protocol::{
    Event, PHY_REGISTER_IMAGE_WORDS, PhyAnalogImageBytes, PhyRegisterImageRequest,
    PhyRegisterImageWords, RejectReason,
};
use static_cell::StaticCell;

/// Busy-host polls one analog read may take; a read completes within a
/// few polls, so exhausting them reports a stuck analog host.
const ANALOG_READ_POLLS: u32 = 10_000;

/// Storage of the image's one shared radio.
static STORAGE: StaticCell<super::SharedRadio> = StaticCell::new();
static RADIO: OnceLock<&'static super::SharedRadio> = OnceLock::new();

/// Place the image's shared radio for the rest of the process and make it
/// observable. Radio hardware has one owner, so a second call cannot occur;
/// it panics in `StaticCell::init`.
pub(crate) fn adopt(radio: super::SharedRadio) -> &'static super::SharedRadio {
    let radio: &'static super::SharedRadio = STORAGE.init(radio);
    let _ = RADIO.init(radio);
    radio
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

/// Read the requested window of the analog image, or reject a window
/// outside the image or wider than one reply, and a read whose analog host
/// stays busy.
pub(crate) async fn read_analog(request: PhyRegisterImageRequest) -> Event {
    let Some(radio) = RADIO.try_get() else {
        return Event::Rejected(RejectReason::InvalidState);
    };
    let mut guard = radio.lock().await;
    let lease = guard.lease();
    let length = lease.phy_analog_image_len();
    let first = usize::from(request.first);
    let count = usize::from(request.count);
    if count > PHY_REGISTER_IMAGE_WORDS || first + count > length {
        return Event::Rejected(RejectReason::InvalidConfiguration);
    }
    let mut values = heapless::Vec::new();
    for index in first..first + count {
        match lease.phy_analog_image(index, ANALOG_READ_POLLS) {
            Some(Ok(value)) => {
                let _ = values.push(value);
            }
            Some(Err(_)) => return Event::Rejected(RejectReason::Busy),
            None => return Event::Rejected(RejectReason::InvalidConfiguration),
        }
    }
    Event::PhyAnalogImage(PhyAnalogImageBytes {
        first: request.first,
        length: length as u16,
        values,
    })
}
