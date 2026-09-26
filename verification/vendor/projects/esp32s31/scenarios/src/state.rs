//! Vendor state the compared cases write but no relation compares.
//!
//! Blobray reports the persistent vendor bytes each compared case writes:
//! writable image segments, session RAM and session allocations. A byte is
//! compared when the case's relation selects it through a projection field,
//! a memory pair or the write timeline. Every byte a case writes without
//! comparing it is either reviewed by a decision below, with its reason, or
//! reported as unprojected in the evidence index. A decision that matches no
//! unprojected byte fails the run.
use crate::harness::{Result, invalid};
use blobray_domain::{ExecutionCase, ExecutionEvidence, LayoutProjection, WrittenRange};
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::{BTreeMap, BTreeSet};

/// A reviewed decision on vendor state the scenarios write without comparing
/// it.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [Place],
}

/// Bytes `start..end` of vendor data `symbol` that the cases of the claim
/// comparing vendor `root` with production `production` write without
/// comparing them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub root: &'static str,
    pub production: &'static str,
    pub symbol: &'static str,
    pub start: u32,
    pub end: u32,
}

/// The claim a place belongs to: vendor root and production entry.
type Claim = (&'static str, &'static str);

const fn place(claim: Claim, symbol: &'static str, start: u32, end: u32) -> Place {
    Place {
        root: claim.0,
        production: claim.1,
        symbol,
        start,
        end,
    }
}

const PARENT: Claim = ("phy_param_track_tot", "open_phy_tracking_trace_parent");
const COMBINED: Claim = ("phy_cal_param_track", "open_phy_calibration_trace_combined");
const RX_GAIN: Claim = (
    "phy_set_rx_gain_table",
    "open_phy_calibration_trace_rx_gain",
);
const CHANNEL: Claim = ("phy_chip_set_chan", "open_phy_channel_trace_state");
const RFPLL_MAINTAIN: Claim = ("phy_rfpll_cap_track_new", "open_phy_rfpll_trace_maintain");
const RFPLL_THERMAL: Claim = ("phy_rfpll_cap_track_new", "open_phy_rfpll_trace_track");
const AP_TSF_START: Claim = (
    "hal_mac_tsf_reset",
    "open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset",
);

/// Reviewed unprojected vendor state.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "`wdev.o` beacon-schedule cursor `BcnSendTick` that a fresh AP TSF epoch clears \
            for `wDev_Get_Next_TBTT`: production keeps the TBTT cursor in the AP engine's beacon \
            state, which each started engine creates empty, not in the MAC TSF leaf",
        places: &[place(AP_TSF_START, "BcnSendTick", 0, 4)],
    },
    Decision {
        reason: "`phy_track.o` static `s_track_result`, named by its section anchor: a debug \
            copy of the current, power, common and transmit reference temperatures, the RFPLL \
            reference and the progress word, all compared as `phy_param` fields of the parent \
            claim; only the exported `phy_debug_get_track_result` reads it, and no vendor \
            library or ROM function calls that",
        places: &[place(PARENT, "0x20000204", 0, 14)],
    },
    Decision {
        reason: "readiness activity edge count of one DC estimate: `phy_iq_est_enable` clears \
            it unconditionally before counting and only `phy_rxdc_est_min` of the same estimate \
            reads it, so no later estimate or caller observes the stored count; the admission \
            it decides is compared through the published DC codes",
        places: &[
            place(RX_GAIN, "phy_param", 0x1ac, 0x1ae),
            place(COMBINED, "phy_param", 0x1ac, 0x1ae),
            place(PARENT, "phy_param", 0x1ac, 0x1ae),
        ],
    },
    Decision {
        reason: "upper halfword of the calibration status word, rewritten unchanged: every \
            vendor status update is a word read-modify-write whose flag bits (0x8, 0x20, 0x80, \
            0x200 and the 0x221 clear) lie in the compared lower halfword",
        places: &[
            place(RX_GAIN, "phy_param", 0xa6, 0xa8),
            place(COMBINED, "phy_param", 0xa6, 0xa8),
            place(PARENT, "phy_param", 0xa6, 0xa8),
        ],
    },
    Decision {
        reason: "RX-gain completion flags (0x80 and 0x200 of the status word) and the common \
            reference temperature `phy_set_rx_gain_table` copies from the current temperature \
            after generating tables: production commits them in its state owner \
            (`apply_rx_gain_init_outcome` and the calibration-tracking commit), which the \
            RX-gain child probe does not run; the combined and parent claims compare both after \
            the same child",
        places: &[
            place(RX_GAIN, "phy_param", 0xa4, 0xa6),
            place(RX_GAIN, "phy_param", 0x190, 0x192),
        ],
    },
    Decision {
        reason: "wide-bandwidth flag `phy_chip_set_chan` derives as `bandwidth != 0` from the \
            compared bandwidth byte; its only reader is ROM `phy_get_pwr_index`, which only \
            `librftest.a` calls, and production has no RF-test power-index path",
        places: &[
            place(CHANNEL, "phy_param", 0x11e, 0x11f),
            place(COMBINED, "phy_param", 0x11e, 0x11f),
            place(PARENT, "phy_param", 0x11e, 0x11f),
        ],
    },
    Decision {
        reason: "reference temperature and RFPLL progress bit the thermal child commits after \
            the correction; the maintenance claim compares the correction's frequency-control \
            effects, and the thermal claim of the same root compares both commits",
        places: &[
            place(RFPLL_MAINTAIN, "phy_param", 0x130, 0x132),
            place(RFPLL_MAINTAIN, "phy_param", 0x1fe, 0x200),
        ],
    },
    Decision {
        reason: "RFPLL tracking reentrancy guard: `phy_rfpll_cap_track_new` sets it after \
            admission and clears it before returning, so the stored byte is unchanged by every \
            completed call; production serializes the child by ownership instead",
        places: &[
            place(RFPLL_MAINTAIN, "phy_param", 0x194, 0x195),
            place(RFPLL_THERMAL, "phy_param", 0x194, 0x195),
            place(PARENT, "phy_param", 0x194, 0x195),
        ],
    },
];

/// One vendor byte, by data symbol and offset; a byte outside every sized
/// data symbol is named by the address of the nearest symbol below it, such
/// as a section-local anchor.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Byte {
    pub symbol: String,
    pub offset: u32,
}

/// Sized data symbols of the vendor executables, by address.
#[derive(Default)]
pub struct Symbols {
    ranges: BTreeMap<u32, (u32, String)>,
    anchors: BTreeSet<u32>,
}

impl Symbols {
    pub fn of(executables: &[&[u8]]) -> Result<Self> {
        let (mut ranges, mut anchors) = (BTreeMap::new(), BTreeSet::new());
        for bytes in executables {
            let file = object::File::parse(*bytes)?;
            for symbol in file.symbols() {
                if !symbol.is_undefined()
                    && let Ok(address) = u32::try_from(symbol.address())
                {
                    anchors.insert(address);
                }
                if symbol.kind() != SymbolKind::Data || symbol.size() == 0 || symbol.is_undefined()
                {
                    continue;
                }
                let (Ok(address), Ok(size), Ok(name)) = (
                    u32::try_from(symbol.address()),
                    u32::try_from(symbol.size()),
                    symbol.name(),
                ) else {
                    continue;
                };
                ranges.entry(address).or_insert((size, name.to_owned()));
            }
        }
        Ok(Self { ranges, anchors })
    }

    pub fn name(&self, address: u32) -> Byte {
        match self.ranges.range(..=address).next_back() {
            Some((start, (size, name))) if address - start < *size => Byte {
                symbol: name.clone(),
                offset: address - start,
            },
            _ => {
                let anchor = self.anchors.range(..=address).next_back().copied();
                let anchor = anchor.unwrap_or(address);
                Byte {
                    symbol: format!("{anchor:#010x}"),
                    offset: address - anchor,
                }
            }
        }
    }
}

/// Vendor byte ranges the relation of `case` compares, or `None` when it
/// compares every vendor write through the write timeline.
fn compared(
    case: &ExecutionCase,
    projections: &[LayoutProjection],
) -> Result<Option<Vec<(u64, u64)>>> {
    let Some(relation) = &case.relation else {
        return Ok(Some(vec![]));
    };
    if relation.events.timeline.writes {
        return Ok(None);
    }
    let mut ranges = vec![];
    if let Some(selected) = &relation.projection {
        let projection = projections
            .iter()
            .find(|p| {
                blobray_application::in_process::projection_ref(p)
                    .ok()
                    .as_ref()
                    == Some(selected)
            })
            .ok_or_else(|| invalid(format!("{}: projection is not reviewed", case.name)))?;
        for field in projection
            .fields
            .iter()
            .filter(|f| f.final_state || f.timeline)
        {
            let length = field.byte_length()?;
            let address = projection.vendor.address(field.vendor, length)?;
            ranges.push((u64::from(address), u64::from(address) + u64::from(length)));
        }
    }
    for pair in &relation.memory {
        let selection = case
            .vendor
            .observe_memory
            .get(usize::from(pair.vendor))
            .ok_or_else(|| invalid(format!("{}: memory pair outside selections", case.name)))?;
        ranges.push((
            u64::from(selection.address),
            u64::from(selection.address) + u64::from(selection.length),
        ));
    }
    Ok(Some(ranges))
}

/// Vendor bytes the compared cases of `records` write, and those a case
/// writes without comparing, for the cases `selected` names.
pub fn written(
    cases: &[ExecutionCase],
    selected: &BTreeSet<u32>,
    records: &[ExecutionEvidence],
    projections: &[LayoutProjection],
) -> Result<(BTreeSet<u32>, BTreeSet<u32>)> {
    let mut ranges: BTreeMap<u32, Vec<WrittenRange>> = BTreeMap::new();
    for record in records {
        if let ExecutionEvidence::Written {
            case,
            replacement: false,
            range,
        } = record
            && selected.contains(case)
        {
            ranges.entry(*case).or_default().push(*range);
        }
    }
    let (mut written, mut unprojected) = (BTreeSet::new(), BTreeSet::new());
    for (case, ranges) in ranges {
        let compared = compared(&cases[case as usize], projections)?;
        for range in ranges {
            for address in u64::from(range.address)..range.end() {
                let address = address as u32;
                written.insert(address);
                let covered = compared.as_ref().is_none_or(|c| {
                    c.iter()
                        .any(|(start, end)| (*start..*end).contains(&u64::from(address)))
                });
                if !covered {
                    unprojected.insert(address);
                }
            }
        }
    }
    Ok((written, unprojected))
}

/// Split the bytes the claim (vendor root, production entry) writes without
/// comparing them into (reviewed, untriaged) under `decisions`.
pub fn classify(
    decisions: &[Decision],
    claim: (&str, &str),
    unprojected: &BTreeSet<Byte>,
) -> (BTreeSet<Byte>, BTreeSet<Byte>) {
    unprojected
        .iter()
        .cloned()
        .partition(|byte| reviewed(decisions, claim, byte).is_some())
}

fn reviewed<'d>(decisions: &'d [Decision], claim: (&str, &str), byte: &Byte) -> Option<&'d Place> {
    decisions.iter().flat_map(|d| d.places).find(|place| {
        (place.root, place.production) == claim
            && byte.symbol == place.symbol
            && (place.start..place.end).contains(&byte.offset)
    })
}

/// Every place of every decision must still review a byte its claim writes
/// without comparing it; `unprojected` holds (root, production, byte) over
/// every claim.
pub fn check(decisions: &[Decision], unprojected: &BTreeSet<(String, String, Byte)>) -> Result<()> {
    let matched: BTreeSet<_> = unprojected
        .iter()
        .filter_map(|(root, production, byte)| reviewed(decisions, (root, production), byte))
        .collect();
    match decisions
        .iter()
        .flat_map(|d| d.places)
        .find(|place| !matched.contains(place))
    {
        Some(place) => Err(invalid(format!(
            "state decision for {}[{:#x}..{:#x}] under {}/{} matches no unprojected byte; \
             the state is compared or no longer written",
            place.symbol, place.start, place.end, place.root, place.production
        ))),
        None => Ok(()),
    }
}

/// Coalesced ranges of `bytes`, by symbol.
pub fn ranges(bytes: &BTreeSet<Byte>) -> Vec<(String, u32, u32)> {
    let mut ranges: Vec<(String, u32, u32)> = vec![];
    for byte in bytes {
        match ranges.last_mut() {
            Some((symbol, offset, length))
                if *symbol == byte.symbol && *offset + *length == byte.offset =>
            {
                *length += 1
            }
            _ => ranges.push((byte.symbol.clone(), byte.offset, 1)),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byte(symbol: &str, offset: u32) -> Byte {
        Byte {
            symbol: symbol.into(),
            offset,
        }
    }

    const DECIDED: &[Decision] = &[Decision {
        reason: "test",
        places: &[Place {
            root: "root",
            production: "entry",
            symbol: "state",
            start: 2,
            end: 4,
        }],
    }];

    #[test]
    fn bytes_are_named_by_sized_symbols_or_the_nearest_anchor() {
        let symbols = Symbols {
            ranges: BTreeMap::from([(0x100, (4, "state".to_owned()))]),
            anchors: BTreeSet::from([0x100, 0x200]),
        };
        assert_eq!(symbols.name(0x103), byte("state", 3));
        assert_eq!(symbols.name(0x104), byte("0x00000100", 4));
        assert_eq!(symbols.name(0x20e), byte("0x00000200", 14));
        assert_eq!(symbols.name(0x80), byte("0x00000080", 0));
    }

    #[test]
    fn decisions_review_their_offsets_and_fail_when_stale() {
        let unprojected = BTreeSet::from([byte("state", 1), byte("state", 3), byte("other", 3)]);
        let (reviewed, untriaged) = classify(DECIDED, ("root", "entry"), &unprojected);
        assert_eq!(reviewed, BTreeSet::from([byte("state", 3)]));
        assert_eq!(untriaged.len(), 2);
        // Another claim's byte is not reviewed.
        assert!(
            classify(DECIDED, ("root", "other"), &unprojected)
                .0
                .is_empty()
        );
        let owned = |entry: &str, bytes: &BTreeSet<Byte>| -> BTreeSet<(String, String, Byte)> {
            bytes
                .iter()
                .map(|b| ("root".to_owned(), entry.to_owned(), b.clone()))
                .collect()
        };
        check(DECIDED, &owned("entry", &unprojected)).unwrap();
        assert!(check(DECIDED, &owned("other", &unprojected)).is_err());
        assert!(
            check(
                DECIDED,
                &owned("entry", &BTreeSet::from([byte("state", 1)]))
            )
            .is_err()
        );
    }

    #[test]
    fn ranges_coalesce_adjacent_bytes_of_one_symbol() {
        let bytes = BTreeSet::from([byte("a", 1), byte("a", 2), byte("a", 4), byte("b", 5)]);
        assert_eq!(
            ranges(&bytes),
            [
                ("a".to_owned(), 1, 2),
                ("a".to_owned(), 4, 1),
                ("b".to_owned(), 5, 1)
            ]
        );
    }
}
