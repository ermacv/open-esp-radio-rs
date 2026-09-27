//! Production calibration words of a captured startup artifact.
use crate::Result;
use crate::calibration_projection::{CALIBRATION_WORDS, snapshot_calibration, snapshot_committed};

/// Committed state words after the calibration projection.
const COMMITTED_WORDS: usize = 8;
/// Tracking progress of a fresh registration; the relation excludes the
/// progress field from the calibration comparison.
const NO_PROGRESS: u16 = 0;

/// The production output bytes of the tracking roots' calibration
/// projection and committed state words, projected from the retained
/// calibration `artifact` the target published.
pub fn output(artifact: &[u8]) -> Result<Vec<u8>> {
    let cache = oer_esp32s31_hil_calibration_artifact::decode(artifact)
        .ok_or("the startup artifact is not a retained calibration of this schema")?;
    let state = oer_esp32s31_phy::validation::retained_calibration_state(*cache.snapshot())
        .map_err(|error| format!("the retained calibration is not replayable: {error:?}"))?;
    let mut calibration = [0; CALIBRATION_WORDS];
    let mut committed = [0; COMMITTED_WORDS];
    snapshot_calibration(&state, &mut calibration);
    snapshot_committed(&state, NO_PROGRESS, &mut committed);
    Ok(calibration
        .iter()
        .chain(&committed)
        .flat_map(|word| word.to_le_bytes())
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn foreign_artifacts_are_rejected() {
        assert!(super::output(b"ORCAL006").is_err());
    }
}
