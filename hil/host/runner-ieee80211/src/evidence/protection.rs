//! How a station target follows BSS protection, judged from independent air
//! frames: every data PPDU it sends to the AP needs a preceding RTS/CTS
//! exchange at the control rate the protection requires, and the RTS NAV must
//! cover the exchange through the AP's response.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    Result,
    evidence::air::{AirFrame, AirPhy, FrameKind, MacAddress},
};

/// Protection advertised by the AP while the frames were captured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Expectation {
    /// ERP Use_Protection: control frames must use a DSSS/HR rate.
    pub erp: bool,
}

/// Frames between the CTS and the data, and between the RTS and the CTS,
/// are SIFS apart; anything slower is not one exchange.
const EXCHANGE_GAP_MICROS: u64 = 1_000;
/// TSFT stamps whole microseconds and PHYs round their durations.
const NAV_TOLERANCE_MICROS: u64 = 2;
const RTS_BYTES: u32 = 20;
const ACK_BYTES: u32 = 14;
const BLOCK_ACK_BYTES: u32 = 32;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProtectionEvidence {
    pub target: Option<String>,
    pub data_ppdus: u32,
    pub protected_ppdus: u32,
    pub target_rts: u32,
    /// RTS frames whose PHY class differs from the protection's control rate
    /// (or whose PHY the capture did not record).
    pub wrong_control_rate: u32,
    /// Protected PPDUs with an observed response, whose NAV was checked.
    pub nav_evaluated: u32,
    /// Protected PPDUs whose RTS NAV ended before the response ended.
    pub nav_short: u32,
    /// The first NAV shortfall, for diagnosis.
    pub first_nav_shortfall_micros: Option<u64>,
}

impl ProtectionEvidence {
    /// Share of data PPDUs preceded by RTS/CTS, in basis points.
    pub fn protected_basis_points(&self) -> u64 {
        u64::from(self.protected_ppdus) * 10_000 / u64::from(self.data_ppdus.max(1))
    }
}

/// Analyze frames captured on the AP's channel. The target is the single
/// station, other than `peer`, that sends data to `bssid`.
pub fn analyze(
    frames: &[AirFrame],
    bssid: MacAddress,
    peer: Option<MacAddress>,
    expectation: Expectation,
) -> Result<ProtectionEvidence> {
    let target = identify_target(frames, bssid, peer)?;
    let mut relevant = frames
        .iter()
        .filter(|frame| relevant(frame, target, bssid))
        .map(|frame| {
            frame
                .mac_time_micros
                .map(|time| (time, frame))
                .ok_or_else(|| "protection evidence requires TSFT on every frame".into())
        })
        .collect::<Result<Vec<_>>>()?;
    relevant.sort_by_key(|(time, _)| *time);

    let mut evidence = ProtectionEvidence {
        target: Some(target.to_string()),
        ..ProtectionEvidence::default()
    };
    for (index, &(time, frame)) in relevant.iter().enumerate() {
        if frame.kind == FrameKind::RTS {
            evidence.target_rts += 1;
            let expected = match frame.phy {
                Some(phy) if expectation.erp => phy.is_dsss(),
                Some(phy) => phy == AirPhy::Ofdm,
                None => false,
            };
            if !expected {
                evidence.wrong_control_rate += 1;
            }
            continue;
        }
        if !frame.kind.is_data()
            || index
                .checked_sub(1)
                .is_some_and(|previous| relevant[previous].1.kind.is_data())
        {
            continue;
        }
        evidence.data_ppdus += 1;
        let exchange = index
            .checked_sub(2)
            .map(|start| (relevant[start], relevant[start + 1]));
        let Some(((rts_time, rts), (cts_time, cts))) = exchange else {
            continue;
        };
        if rts.kind != FrameKind::RTS
            || cts.kind != FrameKind::CTS
            || cts_time - rts_time > EXCHANGE_GAP_MICROS
            || time - cts_time > EXCHANGE_GAP_MICROS
        {
            continue;
        }
        evidence.protected_ppdus += 1;
        let response = relevant[index..]
            .iter()
            .find(|(_, frame)| !frame.kind.is_data())
            .filter(|(_, frame)| {
                frame.kind == FrameKind::BLOCK_ACK || frame.kind == FrameKind::ACK
            });
        let (Some((response_time, response)), Some(duration)) = (response, rts.duration_micros)
        else {
            continue;
        };
        let (Some(rts_airtime), Some(response_airtime)) = (
            control_airtime(rts, RTS_BYTES),
            control_airtime(
                response,
                if response.kind == FrameKind::ACK {
                    ACK_BYTES
                } else {
                    BLOCK_ACK_BYTES
                },
            ),
        ) else {
            continue;
        };
        evidence.nav_evaluated += 1;
        let nav_end = rts_time + rts_airtime + u64::from(duration) + NAV_TOLERANCE_MICROS;
        let response_end = response_time + response_airtime;
        if nav_end < response_end {
            evidence.nav_short += 1;
            evidence
                .first_nav_shortfall_micros
                .get_or_insert(response_end - nav_end);
        }
    }
    Ok(evidence)
}

fn relevant(frame: &AirFrame, target: MacAddress, bssid: MacAddress) -> bool {
    let to_target = frame.receiver == Some(target);
    match frame.kind {
        FrameKind::RTS => frame.transmitter == Some(target) && frame.receiver == Some(bssid),
        FrameKind::CTS | FrameKind::ACK | FrameKind::BLOCK_ACK => to_target,
        kind => {
            kind.is_data() && frame.transmitter == Some(target) && frame.receiver == Some(bssid)
        }
    }
}

fn identify_target(
    frames: &[AirFrame],
    bssid: MacAddress,
    peer: Option<MacAddress>,
) -> Result<MacAddress> {
    let mut senders = BTreeMap::<MacAddress, u32>::new();
    for frame in frames {
        if let Some(transmitter) = frame.transmitter
            && frame.kind.is_data()
            && frame.receiver == Some(bssid)
            && Some(transmitter) != peer
        {
            *senders.entry(transmitter).or_default() += 1;
        }
    }
    let total = senders.values().sum::<u32>();
    let (target, count) = senders
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .ok_or("the air capture holds no station data sent to the AP")?;
    if u64::from(count) * 10 < u64::from(total) * 9 {
        return Err(format!(
            "no single station dominates the data sent to the AP ({count} of {total} frames)"
        )
        .into());
    }
    Ok(target)
}

/// Airtime of a control frame of `bytes` at the captured rate, including the
/// PLCP preamble and header and, for ERP-OFDM, the signal extension.
fn control_airtime(frame: &AirFrame, bytes: u32) -> Option<u64> {
    let rate_kbps = u64::from(frame.rate_kbps?);
    if rate_kbps == 0 {
        return None;
    }
    let bits = u64::from(bytes) * 8;
    match frame.phy? {
        phy if phy.is_dsss() => {
            let preamble = if frame.short_preamble? && rate_kbps > 1_000 {
                96
            } else {
                192
            };
            Some(preamble + (bits * 1_000).div_ceil(rate_kbps))
        }
        AirPhy::Ofdm => {
            let bits_per_symbol = rate_kbps * 4 / 1_000;
            let symbols = (16 + bits + 6).div_ceil(bits_per_symbol);
            Some(20 + symbols * 4 + 6)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
