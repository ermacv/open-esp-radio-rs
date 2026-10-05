//! HIL image classes: each class's identity, the Cargo features it is
//! built with, and the keys an image of it reports on each chip.
//!
//! A scenario names the class it needs; the image builder builds a class
//! from these recipes, and the runner identifies a flashed image by the keys
//! it reports.

pub mod agent;
mod class;
mod features;
mod keys;

pub use class::ImageClass;
pub use features::FeatureDelta;
pub use keys::classify_flashed;

/// The network implementation every staged image class links: owned
/// Xarxa/Embassy, the maintained owner-transfer forks the manifests
/// declare. Image records and build directories name it.
pub const NETWORK: &str = "owned-xarxa";

/// The Cargo feature that selects [`NETWORK`] in an image.
pub const NETWORK_FEATURE: &str = "owned-network";

#[cfg(test)]
mod tests {
    #[test]
    fn a_diagnostic_class_is_named_diagnostic() {
        for class in super::ImageClass::ALL {
            assert_eq!(
                class.diagnostic(),
                class.id().starts_with("diagnostic-"),
                "{}",
                class.id()
            );
        }
    }
}
