//! This image's Cargo features and the keys it reports for them.

use embassy_sync::once_lock::OnceLock;
#[cfg(feature = "open-radio-hil")]
use oer_hil_protocol::Message;
use oer_hil_target_core::image_capabilities::ImageKeys;

include!(concat!(env!("OUT_DIR"), "/enabled_features.rs"));

/// The keys of this image: the same function of its Cargo features the host
/// derives each image class's expected keys with.
pub(crate) fn keys() -> &'static ImageKeys {
    static KEYS: OnceLock<ImageKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        oer_hil_target_core::image_capabilities::image_keys(&|feature| {
            ENABLED_FEATURES.contains(&feature)
        })
    })
}

/// Whether this image has message or property `M`.
#[cfg(feature = "open-radio-hil")]
pub(crate) fn has<M: Message>() -> bool {
    keys().binary_search(&M::KEY).is_ok()
}
