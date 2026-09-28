//! This image's Cargo features and the capabilities it reports for them.

include!(concat!(env!("OUT_DIR"), "/enabled_features.rs"));

/// The capabilities of this image: the same function of its Cargo features
/// the host derives each image class's expected capabilities with.
pub(crate) fn feature_capabilities() -> oer_hil_protocol::FeatureCapabilities {
    oer_hil_target_core::image_capabilities::feature_capabilities(&|feature| {
        ENABLED_FEATURES.contains(&feature)
    })
}
