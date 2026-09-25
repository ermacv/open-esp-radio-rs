//! Retain terminal MAC receipts separately from UDP and unfinished TX work.
use crate::Result;
use open_esp_radio_hil_protocol::StationTxTerminalEvidence;
use std::{fs, path::Path};

pub(super) fn retain(output: &Path, value: StationTxTerminalEvidence) -> Result<()> {
    fs::write(
        output.join("station-tx-terminal.json"),
        serde_json::to_vec_pretty(&value)?,
    )?;
    validate(value)
}

fn validate(value: StationTxTerminalEvidence) -> Result<()> {
    if value.invalid_statuses != 0
        || value.mpdus != value.acknowledged.wrapping_add(value.unacknowledged)
    {
        return Err("inconsistent station terminal TX receipt counters".into());
    }
    Ok(())
}

pub(super) fn markdown(value: StationTxTerminalEvidence) -> String {
    format!(
        "- Terminal station exchanges: `{}`; MPDUs acknowledged / unacknowledged: `{}` / `{}`; ordinary fallback recovered / failed: `{}` / `{}`. Live exchanges are excluded; missing ACK does not prove missing UDP. [Receipts](station-tx-terminal.json)\n",
        value.exchanges,
        value.acknowledged,
        value.unacknowledged,
        value.ordinary_recovered,
        value.ordinary_failed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unacknowledged_is_valid_evidence_but_broken_accounting_is_not() {
        let mut value = StationTxTerminalEvidence {
            exchanges: 1,
            mpdus: 3,
            acknowledged: 2,
            unacknowledged: 1,
            ..Default::default()
        };
        assert!(validate(value).is_ok());
        value.invalid_statuses = 1;
        assert!(validate(value).is_err());
        value.invalid_statuses = 0;
        value.acknowledged = 3;
        assert!(validate(value).is_err());
    }
}
