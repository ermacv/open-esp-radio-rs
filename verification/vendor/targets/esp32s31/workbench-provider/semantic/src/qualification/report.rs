//! ESP32-S31 conversions for the generic qualification report model.

pub use open_radio_vendor_semantics::{
    QualificationArtifact, QualificationCase, QualificationDifference, QualificationReport,
    QualificationStateFootprint, QualificationSummary,
};

impl From<super::StateFootprintStats> for QualificationStateFootprint {
    fn from(value: super::StateFootprintStats) -> Self {
        Self {
            read_bytes: value.read_bytes,
            written_bytes: value.written_bytes,
            classified_ranges: value.classified_ranges,
        }
    }
}
