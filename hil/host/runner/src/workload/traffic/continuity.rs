//! Delivery continuity is independent of average-throughput and radio gates.
use crate::Result;
use open_esp_radio_hil_protocol::TransportEvidence;

pub(super) fn require_rx_silence(limit_ms: Option<u32>, evidence: TransportEvidence) -> Result<()> {
    let Some(limit_ms) = limit_ms else {
        return Ok(());
    };
    let observed = evidence
        .rx_maximum_silence_micros
        .ok_or("missing typed complete-window RX silence evidence")?;
    if observed > evidence.elapsed_micros {
        return Err("RX silence exceeds its measured observation window".into());
    }
    if observed > u64::from(limit_ms) * 1_000 {
        return Err(format!(
            "RX continuity failed: maximum silence {observed} us exceeds {limit_ms} ms"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn throughput_cannot_hide_missing_or_overlong_silence() {
        let mut evidence = TransportEvidence {
            rx_maximum_silence_micros: Some(2_995_274),
            rx_bytes: 58_000_000,
            tx_bytes: 78_000_000,
            rx_units: 48_333,
            tx_units: 65_000,
            elapsed_micros: 12_000_000,
            transport_errors: 0,
        };
        assert!(require_rx_silence(Some(250), evidence).is_err());
        evidence.rx_maximum_silence_micros = Some(250_000);
        assert!(require_rx_silence(Some(250), evidence).is_ok());
        evidence.rx_maximum_silence_micros = None;
        assert!(require_rx_silence(Some(250), evidence).is_err());
        assert!(require_rx_silence(None, evidence).is_ok());
        evidence.rx_maximum_silence_micros = Some(12_000_001);
        assert!(require_rx_silence(Some(20_000), evidence).is_err());
    }
}
