//! ESP32-S31 conversions for the generic semantic-verification report model.

pub use open_radio_vendor_semantics::{
    SemanticVerificationArtifact, SemanticVerificationCase, SemanticVerificationDifference,
    SemanticVerificationReport, SemanticVerificationStateFootprint, SemanticVerificationSummary,
};

impl From<super::StateFootprintStats> for SemanticVerificationStateFootprint {
    fn from(value: super::StateFootprintStats) -> Self {
        Self {
            read_bytes: value.read_bytes,
            written_bytes: value.written_bytes,
            classified_ranges: value.classified_ranges,
        }
    }
}
