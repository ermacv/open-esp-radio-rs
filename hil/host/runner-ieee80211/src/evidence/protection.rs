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

/// The RTS and its CTS are SIFS apart; anything slower is not one exchange.
const EXCHANGE_GAP_MICROS: u64 = 1_000;
/// The response follows the stamped MPDU of a PPDU by at most that MPDU's
/// airtime and SIFS.
const RESPONSE_WINDOW_MICROS: u64 = 1_000;
/// TSFT stamps whole microseconds and PHYs round their durations.
const NAV_TOLERANCE_MICROS: u64 = 2;
const RTS_BYTES: u32 = 20;
const ACK_BYTES: u32 = 14;
const BLOCK_ACK_BYTES: u32 = 32;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProtectionEvidence {
    pub target: Option<String>,
    pub data_ppdus: u32,
    /// Data PPDUs immediately preceded, within its NAV, by the target's RTS
    /// or by a CTS to the target.
    pub protected_ppdus: u32,
    /// Protected PPDUs whose RTS the observer lost (only its CTS was seen).
    pub rts_unobserved: u32,
    /// Protected PPDUs whose CTS the observer lost (only the RTS was seen).
    pub cts_unobserved: u32,
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
    let timeline = timeline(frames, target, bssid)?;
    let mut evidence = ProtectionEvidence {
        target: Some(target.to_string()),
        ..ProtectionEvidence::default()
    };
    for (index, &(time, event)) in timeline.iter().enumerate() {
        let rts = match event {
            Event::Rts(frame) => {
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
            Event::Ppdu => {
                evidence.data_ppdus += 1;
                // Either observed half of the exchange shows it: a CTS to the
                // target answers only the target's RTS. The observer loses
                // single control frames, so both halves are not required.
                let previous = index.checked_sub(1).map(|previous| timeline[previous]);
                let earlier = index.checked_sub(2).map(|earlier| timeline[earlier]);
                match (earlier, previous) {
                    (Some((rts_time, Event::Rts(rts))), Some((cts_time, Event::Cts(cts))))
                        if within_nav(cts_time, cts, time)
                            && cts_time - rts_time <= EXCHANGE_GAP_MICROS =>
                    {
                        Some((rts_time, rts))
                    }
                    (_, Some((cts_time, Event::Cts(cts)))) if within_nav(cts_time, cts, time) => {
                        evidence.rts_unobserved += 1;
                        None
                    }
                    (_, Some((rts_time, Event::Rts(rts)))) if within_nav(rts_time, rts, time) => {
                        evidence.cts_unobserved += 1;
                        Some((rts_time, rts))
                    }
                    _ => continue,
                }
            }
            Event::UntimedPpdu => {
                evidence.data_ppdus += 1;
                continue;
            }
            Event::Cts(_) | Event::Response(_) => continue,
        };
        evidence.protected_ppdus += 1;
        // The NAV is judged where the RTS that set it was observed.
        let Some((rts_time, rts)) = rts else {
            continue;
        };
        let Some(duration) = rts.duration_micros.map(u64::from) else {
            continue;
        };
        let response =
            timeline[index + 1..]
                .first()
                .and_then(|&(response_time, event)| match event {
                    Event::Response(frame) if response_time - time <= RESPONSE_WINDOW_MICROS => {
                        Some((response_time, frame))
                    }
                    _ => None,
                });
        let Some((response_time, response)) = response else {
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
        let nav_end = rts_time + rts_airtime + duration + NAV_TOLERANCE_MICROS;
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

#[derive(Clone, Copy, Debug)]
enum Event<'a> {
    Rts(&'a AirFrame),
    Cts(&'a AirFrame),
    /// A data PPDU, timed by the one MPDU the observer stamped.
    Ppdu,
    /// A data PPDU whose stamped MPDU the observer lost. It counts as a PPDU
    /// but can never be shown to be protected.
    UntimedPpdu,
    Response(&'a AirFrame),
}

/// The target's exchanges ordered by TSFT. Capture order is not air order:
/// the observer delivers control frames and A-MPDUs on different paths, and
/// stamps TSFT on a single MPDU of each A-MPDU.
fn timeline<'a>(
    frames: &'a [AirFrame],
    target: MacAddress,
    bssid: MacAddress,
) -> Result<Vec<(u64, Event<'a>)>> {
    let mut timeline = Vec::new();
    let mut aggregates = BTreeMap::<u32, Option<u64>>::new();
    let mut untimed = 0;
    for frame in frames.iter().filter(|frame| relevant(frame, target, bssid)) {
        let event = match frame.kind {
            FrameKind::RTS => Event::Rts(frame),
            FrameKind::CTS => Event::Cts(frame),
            FrameKind::ACK | FrameKind::BLOCK_ACK => Event::Response(frame),
            _ => {
                match frame.ampdu_reference {
                    Some(reference) => {
                        let stamp = aggregates.entry(reference).or_default();
                        *stamp = stamp.or(frame.mac_time_micros);
                    }
                    None => match frame.mac_time_micros {
                        Some(time) => timeline.push((time, Event::Ppdu)),
                        None => untimed += 1,
                    },
                }
                continue;
            }
        };
        timeline.push((timed(frame)?, event));
    }
    for stamp in aggregates.into_values() {
        match stamp {
            Some(time) => timeline.push((time, Event::Ppdu)),
            None => untimed += 1,
        }
    }
    timeline.sort_by_key(|(time, _)| *time);
    // Untimed PPDUs precede everything, so no exchange is attributed to them.
    timeline.splice(0..0, (0..untimed).map(|_| (0, Event::UntimedPpdu)));
    Ok(timeline)
}

/// A PPDU stamped at `time` lies inside the NAV a control frame set.
fn within_nav(control_time: u64, control: &AirFrame, time: u64) -> bool {
    control
        .duration_micros
        .is_some_and(|duration| time - control_time <= u64::from(duration))
}

/// The TSFT of a frame whose time the analysis measures.
fn timed(frame: &AirFrame) -> Result<u64> {
    frame.mac_time_micros.ok_or_else(|| {
        format!(
            "protection evidence requires TSFT on {:#06x} frames",
            frame.kind.0
        )
        .into()
    })
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
