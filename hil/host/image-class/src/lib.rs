//! HIL image classes: each class's identity, the Cargo features it is
//! built with, and the keys an image of it reports on each chip.
//!
//! A scenario names the class it needs; the image builder builds a class
//! from these recipes, and the runner identifies a flashed image by the keys
//! it reports.

use std::collections::BTreeSet;

use oer_hil_protocol::base::{Hello, ImageKeyPage};
use oer_hil_protocol::{Key, Message};

mod class;
mod features;
mod keys;

pub use class::ImageClass;
pub use features::FeatureDelta;
pub use keys::classify_flashed;

/// This package's directory in the repository: a change to a class's
/// recipe changes its image, so the image builder counts it among its own
/// sources.
pub const REPOSITORY_DIRECTORY: &str = "hil/host/image-class";

/// What the device's image serves: the keys of its endpoints, of the
/// messages it sends and of its properties.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct DeviceImageKeys {
    #[serde(serialize_with = "keys_as_paths")]
    keys: BTreeSet<Key>,
    pub maximum_payload_bytes: u16,
    pub maximum_wire_frame_bytes: u16,
}

fn keys_as_paths<S: serde::Serializer>(
    keys: &BTreeSet<Key>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(keys.iter().map(|key| {
        match oer_hil_protocol::registry().find(|info| info.key == *key) {
            Some(info) => info.path.to_owned(),
            None => key.to_string(),
        }
    }))
}

impl DeviceImageKeys {
    /// The set a Hello announces, from its pages.
    pub fn assemble(hello: Hello, pages: &[ImageKeyPage]) -> Result<Self, String> {
        let keys: Vec<Key> = pages
            .iter()
            .flat_map(|page| page.keys.iter().copied())
            .collect();
        if keys.len() != usize::from(hello.keys) {
            return Err(format!(
                "image key pages hold {} keys, the hello announced {}",
                keys.len(),
                hello.keys
            ));
        }
        if oer_hil_protocol::base::digest(&keys) != hello.keys_digest {
            return Err("image key pages do not match the hello's digest".into());
        }
        Ok(Self {
            keys: keys.into_iter().collect(),
            maximum_payload_bytes: hello.maximum_payload_bytes,
            maximum_wire_frame_bytes: hello.maximum_wire_frame_bytes,
        })
    }

    /// A set of `keys`, for image classes the host derives.
    pub fn of_keys(keys: impl IntoIterator<Item = Key>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
            maximum_payload_bytes: 0,
            maximum_wire_frame_bytes: oer_hil_protocol::MAX_WIRE_FRAME_BYTES as u16,
        }
    }

    /// Whether the image has message or property `M`, with this host's
    /// schema of it.
    pub fn has<M: Message>(&self) -> bool {
        self.keys.contains(&M::KEY)
    }

    pub fn keys(&self) -> &BTreeSet<Key> {
        &self.keys
    }

    /// Whether the image reports the same keys as `other`.
    pub fn same_keys(&self, other: &Self) -> bool {
        self.keys == other.keys
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_repository_directory_is_this_package() {
        assert!(env!("CARGO_MANIFEST_DIR").ends_with(super::REPOSITORY_DIRECTORY));
    }

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
