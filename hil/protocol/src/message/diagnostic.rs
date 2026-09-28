//! Diagnostic build features an image reports in its capabilities.
//!
//! A diagnostic image class is identified by the diagnostic features compiled
//! into it. The set travels as a sequence of typed feature identities, so a
//! new diagnostic class adds a variant without changing the protocol version,
//! and a feature this host does not know fails deserialization rather than
//! being ignored.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{SeqAccess, Visitor},
    ser::SerializeSeq,
};

/// One diagnostic build feature.
///
/// The variant index is the wire identity and the set's bit: append new
/// variants only; never reorder or remove one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticFeature {
    /// Physical RX allocation lifetime observations.
    RxOwnership,
    /// The Wi-Fi system's diagnostics, including the station's exit
    /// evidence, without the other driver observers: a performance-like
    /// image whose timing still reproduces saturated-traffic failures.
    StationExit,
    /// The program-counter profile: a sampling interrupt on each hart.
    PcProfile,
}

impl DiagnosticFeature {
    pub const ALL: [Self; 3] = [Self::RxOwnership, Self::StationExit, Self::PcProfile];

    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// The set of diagnostic features compiled into an image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticFeatures(u16);

impl DiagnosticFeatures {
    pub const fn empty() -> Self {
        Self(0)
    }

    /// This set with `feature` present exactly when `enabled`.
    pub const fn with(self, feature: DiagnosticFeature, enabled: bool) -> Self {
        if enabled {
            Self(self.0 | feature.bit())
        } else {
            Self(self.0 & !feature.bit())
        }
    }

    pub const fn contains(self, feature: DiagnosticFeature) -> bool {
        self.0 & feature.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = DiagnosticFeature> {
        DiagnosticFeature::ALL
            .into_iter()
            .filter(move |feature| self.contains(*feature))
    }
}

impl FromIterator<DiagnosticFeature> for DiagnosticFeatures {
    fn from_iter<I: IntoIterator<Item = DiagnosticFeature>>(features: I) -> Self {
        features
            .into_iter()
            .fold(Self::empty(), |set, feature| set.with(feature, true))
    }
}

impl Serialize for DiagnosticFeatures {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.iter().count()))?;
        for feature in self.iter() {
            sequence.serialize_element(&feature)?;
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for DiagnosticFeatures {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FeaturesVisitor;

        impl<'de> Visitor<'de> for FeaturesVisitor {
            type Value = DiagnosticFeatures;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence of distinct diagnostic features")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut features = DiagnosticFeatures::empty();
                while let Some(feature) = sequence.next_element::<DiagnosticFeature>()? {
                    if features.contains(feature) {
                        return Err(serde::de::Error::custom("repeated diagnostic feature"));
                    }
                    features = features.with(feature, true);
                }
                Ok(features)
            }
        }

        deserializer.deserialize_seq(FeaturesVisitor)
    }
}

#[cfg(test)]
mod tests;
