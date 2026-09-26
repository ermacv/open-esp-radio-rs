//! Wi-Fi MAC HAL leaves of the pinned `libpp.a` against compiled production.
//!
//! Each leaf is one vendor function and the production probe that runs its
//! HAL counterpart. Every case runs both over the whole radio register block
//! retained from one fill pattern, so every register bit the leaf reads takes
//! both values across the fills, and compares every register effect exactly
//! and, where the leaf returns one, the return word.
use crate::harness::{Arg, Buffer, Input, Result, case, direct, invalid, known};
use crate::layout::{INPUT, radio_aperture};
use crate::phy::{image_layout, select};
use crate::session::{Session, image_symbol_id, request};
use blobray_domain::{
    ComparisonVerdict, EffectContractRef, EffectDisposition, EffectPattern, EffectRule,
    EffectSelector, EffectValue, ExecutionCase, ExecutionGoal, ExecutionSymbol, ExecutionTarget,
    LinkRequest, ObjectId, ObjectLocation, SessionReset,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// SHA-256 of the pinned `libpp.a`.
pub const LIBPP_SHA: &str = "f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e";
/// Input index of the vendor Wi-Fi firmware.
const PHY_SDK_INPUT: u64 = 3;
/// Radio register fills: all clear, all set and two alternating patterns.
pub const LEAF_FILLS: [u8; 4] = [0x00, 0xff, 0x5a, 0xa5];
/// Guest events one leaf case may record.
const LEAF_EVENTS: u32 = 1 << 10;
/// Logical transmit queues the production HAL admits.
const QUEUES: &[u32] = &[0, 1, 2, 3];
/// Event masks: none, the lowest bit, an alternating pattern and all bits.
const EVENT_MASKS: &[u32] = &[0, 1, 0x5a5a_a5a5, u32::MAX];

/// Private inputs and budget of the MAC scenario.
pub struct MacOptions {
    pub binary: PathBuf,
    pub libpp: PathBuf,
    pub rom: PathBuf,
    /// Authenticated vendor Wi-Fi firmware: supplies the network-stack data
    /// and logging symbols that code after a compared prefix references.
    pub phy_sdk: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: crate::harness::Budget,
    pub patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// Values one parameter takes: ABI words, or six-byte addresses the leaf
/// reads through a pointer to a buffer at `INPUT`.
#[derive(Clone, Copy)]
pub enum Domain {
    Words(&'static [u32]),
    Addresses(&'static [[u8; 6]]),
}

impl Domain {
    fn len(self) -> usize {
        match self {
            Self::Words(values) => values.len(),
            Self::Addresses(values) => values.len(),
        }
    }
}

/// One vendor leaf, its production probe and the argument domain of each
/// parameter, in ABI order; the cases are their product.
pub struct Leaf {
    pub vendor: &'static str,
    pub probe: &'static str,
    pub parameters: &'static [(&'static str, Domain)],
    pub returns: bool,
    /// Full fences production adds to order the leaf's register edge
    /// against surrounding memory and device accesses.
    pub ordering_fences: u32,
    /// A bounded feature: the vendor side stops before calling this
    /// function, and only the prefix up to that call is compared.
    pub prefix_until: Option<&'static str>,
}

const fn leaf(
    vendor: &'static str,
    probe: &'static str,
    parameters: &'static [(&'static str, Domain)],
    returns: bool,
) -> Leaf {
    Leaf {
        vendor,
        probe,
        parameters,
        returns,
        ordering_fences: 0,
        prefix_until: None,
    }
}

/// A leaf whose production counterpart adds `fences` ordering fences.
const fn ordered(leaf: Leaf, fences: u32) -> Leaf {
    Leaf {
        ordering_fences: fences,
        ..leaf
    }
}

/// A leaf compared only up to the vendor's call of `callee`.
const fn prefix(leaf: Leaf, callee: &'static str) -> Leaf {
    Leaf {
        prefix_until: Some(callee),
        ..leaf
    }
}

/// Full-fence predecessor and successor sets: device input, output, memory
/// reads and writes.
const FULL_FENCE: u8 = 0xf;

/// Logical MAC interface contexts: station, access point and two others.
const INTERFACES: &[u32] = &[0, 1, 2, 3];
/// Interface addresses: ascending bytes, all clear, all set with the group
/// bit, and an alternating pattern.
const ADDRESSES: &[[u8; 6]] = &[
    [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
    [0; 6],
    [0xff; 6],
    [0x5a, 0xa5, 0x5a, 0xa5, 0x5a, 0xa5],
];

/// Receive-descriptor base addresses: null, an internal-RAM word, an
/// alternating pattern and all bits.
const RX_BASES: &[u32] = &[0, 0x4080_0000, 0x5a5a_a5a4, u32::MAX];
/// Clear-channel-assessment selector values of the two-bit field.
const CCA: &[u32] = &[0, 1, 2, 3];
/// The only AP TSF reset selector production implements: a fresh epoch.
const TSF_FRESH_EPOCH: &[u32] = &[0];
/// The only queue-state clear selector production implements: ordinary
/// transmit completion.
const COMPLETION_CLEAR: &[u32] = &[2];

/// Boolean flag words.
const FLAGS: &[u32] = &[0, 1];
/// Words the vendor ignores: zero and a pattern.
const IGNORED: &[u32] = &[0, 0xa5a5_5a5a];
/// RTS thresholds in bytes, including one the 16-bit field truncates.
const RTS_THRESHOLDS: &[u32] = &[0, 1, 0xffff, 0x1_2345];

/// Every compared leaf.
pub const LEAVES: &[Leaf] = &[
    leaf(
        "hal_disable_softap_tsf",
        "open_libpp_ap_tsf_trace_hal_disable_softap_tsf",
        &[],
        false,
    ),
    leaf(
        "hal_mac_tsf_reset",
        "open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset",
        &[("selector", Domain::Words(TSF_FRESH_EPOCH))],
        false,
    ),
    leaf(
        "hal_mac_interrupt_get_event",
        "open_libpp_trace_hal_mac_interrupt_ret_get_event",
        &[],
        true,
    ),
    ordered(
        leaf(
            "hal_mac_interrupt_clr_event",
            "open_libpp_trace_hal_mac_interrupt_ret_clr_event",
            &[("events", Domain::Words(EVENT_MASKS))],
            false,
        ),
        1,
    ),
    leaf(
        "hal_pwr_interrupt_get_event",
        "open_libpp_power_irq_trace_hal_pwr_interrupt_get_event",
        &[],
        true,
    ),
    ordered(
        leaf(
            "hal_pwr_interrupt_clr_event",
            "open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event",
            &[("events", Domain::Words(EVENT_MASKS))],
            false,
        ),
        1,
    ),
    leaf(
        "hal_mac_rx_disable",
        "open_libpp_rx_trace_hal_mac_rx_disable",
        &[],
        false,
    ),
    leaf(
        "hal_mac_rx_enable",
        "open_libpp_rx_trace_hal_mac_rx_enable",
        &[],
        false,
    ),
    leaf(
        "hal_mac_rx_set_base",
        "open_libpp_rx_trace_hal_mac_rx_set_base",
        &[("address", Domain::Words(RX_BASES))],
        false,
    ),
    leaf(
        "hal_mac_rx_is_dscr_reload",
        "open_libpp_rx_trace_hal_mac_rx_is_dscr_reload",
        &[],
        true,
    ),
    leaf(
        "hal_mac_rx_set_dscr_reload",
        "open_libpp_rx_trace_hal_mac_rx_set_dscr_reload",
        &[],
        false,
    ),
    leaf(
        "hal_mac_tx_set_cca",
        "open_libpp_tx_trace_hal_mac_tx_set_cca",
        &[("value", Domain::Words(CCA))],
        true,
    ),
    leaf(
        "hal_mac_get_txq_in_trig_flow_state",
        "open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state",
        &[],
        true,
    ),
    leaf(
        "hal_mac_is_txq_enabled",
        "open_libpp_tx_trace_hal_mac_is_txq_enabled",
        &[("queue", Domain::Words(QUEUES))],
        true,
    ),
    leaf(
        "hal_mac_is_txq_valid",
        "open_libpp_tx_trace_hal_mac_is_txq_valid",
        &[("queue", Domain::Words(QUEUES))],
        true,
    ),
    leaf(
        "hal_mac_set_txq_invalid",
        "open_libpp_tx_trace_hal_mac_set_txq_invalid",
        &[("queue", Domain::Words(QUEUES))],
        false,
    ),
    leaf(
        "hal_mac_txq_disable",
        "open_libpp_tx_trace_hal_mac_txq_disable",
        &[("queue", Domain::Words(QUEUES))],
        false,
    ),
    leaf(
        "hal_mac_txq_disable",
        "open_ordinary_tx_ownership_disable",
        &[("queue", Domain::Words(QUEUES))],
        false,
    ),
    leaf(
        "hal_mac_clr_txq_state",
        "open_ordinary_tx_ownership_acknowledge",
        &[
            ("selector", Domain::Words(COMPLETION_CLEAR)),
            ("queue", Domain::Words(QUEUES)),
        ],
        false,
    ),
    leaf(
        "hal_disable_sta_beacon_filter",
        "open_wifi_sta_trace_hal_disable_sta_beacon_filter",
        &[],
        false,
    ),
    leaf(
        "hal_mac_set_addr",
        "open_libpp_interface_trace_hal_mac_set_addr",
        &[
            ("interface", Domain::Words(INTERFACES)),
            ("address", Domain::Addresses(ADDRESSES)),
        ],
        false,
    ),
    leaf(
        "hal_mac_set_bssid",
        "open_libpp_interface_trace_hal_mac_set_bssid",
        &[
            ("interface", Domain::Words(INTERFACES)),
            ("address", Domain::Addresses(ADDRESSES)),
        ],
        false,
    ),
    leaf(
        "hal_he_set_tx_protection",
        "open_tx_protection_control_configure_rts",
        &[
            ("queue", Domain::Words(QUEUES)),
            ("enabled", Domain::Words(FLAGS)),
            ("_unused", Domain::Words(IGNORED)),
            ("duration_threshold", Domain::Words(FLAGS)),
            ("threshold_bytes", Domain::Words(RTS_THRESHOLDS)),
        ],
        false,
    ),
    leaf(
        "hal_he_disable_rts_threshold",
        "open_tx_protection_control_disable_he_threshold",
        &[],
        false,
    ),
    prefix(
        ordered(
            leaf(
                "hal_mac_txq_enable",
                "open_ordinary_tx_ownership_publish",
                &[("queue", Domain::Words(QUEUES))],
                false,
            ),
            2,
        ),
        "GetAccess",
    ),
];

/// Linked `libpp.a` image with its captured roots and both execution targets.
pub struct Mac {
    pub session: Session,
    pub roots: BTreeMap<String, u32>,
    pub image_object: ObjectId,
    pub vendor: ExecutionTarget,
    pub production: ExecutionTarget,
}

impl std::ops::Deref for Mac {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}
impl std::ops::DerefMut for Mac {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

impl Mac {
    pub fn new(options: &MacOptions) -> Result<Self> {
        let inputs = [
            Input {
                role: "libpp",
                path: &options.libpp,
                sha256: Some(LIBPP_SHA),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(crate::ROM_SHA),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
            crate::phy::phy_sdk_input(&options.phy_sdk),
        ];
        let session = Session::start(
            &options.binary,
            &options.output,
            options.budget,
            &inputs,
            "Wi-Fi MAC HAL leaf comparison",
            &options.patches,
        )?;
        // The first leaf is the link entry; the others are further roots.
        let mut vendors: Vec<&str> = LEAVES
            .iter()
            .map(|l| l.vendor)
            .filter(|v| *v != LEAVES[0].vendor)
            .collect();
        vendors.sort_unstable();
        vendors.dedup();
        let roots = vendors
            .iter()
            .map(|v| select(&session, 0, v))
            .collect::<Result<Vec<_>>>()?;
        let link = LinkRequest {
            companions: vec![],
            revision: Some(session.revision.clone()),
            inputs: vec![0],
            entry: select(&session, 0, LEAVES[0].vendor)?,
            roots,
            layout: image_layout(),
        };
        let linked = session.link(
            &link,
            &options.linker,
            LEAVES[0].vendor,
            &[crate::layout::ROM_INPUT, PHY_SDK_INPUT],
        )?;
        let (vendor, production) = session.targets(&linked.image)?;
        Ok(Self {
            image_object: ObjectId {
                artifact: linked.manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            roots: linked.roots,
            vendor,
            production,
            session,
        })
    }

    /// The reviewed contract of a leaf whose production adds ordering
    /// fences: exactly that many full fences, every other effect compared
    /// exactly.
    fn ordering_contract(&mut self, leaf: &Leaf) -> Result<EffectContractRef> {
        let vendor = self.session.image_endpoint(
            &self.vendor,
            &self.image_object,
            leaf.vendor,
            self.roots[leaf.vendor],
        )?;
        let production = self.session.input_endpoint(2, leaf.probe)?;
        let rule = EffectRule {
            name: "device-ordering-fence".into(),
            vendor: None,
            replacement: Some(EffectPattern {
                selector: EffectSelector::Fence {
                    predecessor: FULL_FENCE,
                    successor: FULL_FENCE,
                },
                value: EffectValue::Any,
                followed_by: None,
            }),
            disposition: EffectDisposition::Added,
            min_occurrences: leaf.ordering_fences,
            max_occurrences: leaf.ordering_fences,
            reason: "production orders the register edge against surrounding memory and device \
                accesses; the vendor leaves ordering to its caller"
                .into(),
        };
        let applicability = "one MAC register edge over retained radio registers";
        self.session.review_effects(
            &format!("{}-effects", leaf.probe),
            &format!("esp32s31.mac.{}.effects", leaf.vendor),
            crate::contracts::phy_contract(vendor, production, vec![rule], applicability),
            "every register effect compares exactly; production adds ordering fences",
        )
    }

    /// Both sides of `leaf` for every argument combination and fill.
    fn cases(&mut self, leaf: &Leaf) -> Result<Vec<ExecutionCase>> {
        let effects = if leaf.ordering_fences != 0 {
            Some(self.ordering_contract(leaf)?)
        } else {
            None
        };
        // Every combination of one value index per parameter.
        let mut combinations: Vec<Vec<usize>> = vec![vec![]];
        for (_, domain) in leaf.parameters {
            combinations = combinations
                .into_iter()
                .flat_map(|prefix| {
                    (0..domain.len()).map(move |index| {
                        let mut indices = prefix.clone();
                        indices.push(index);
                        indices
                    })
                })
                .collect();
        }
        let mut rows = vec![];
        for indices in &combinations {
            let (mut words, mut memory, mut arguments, mut label) =
                (vec![], vec![], vec![], String::new());
            for ((name, domain), index) in leaf.parameters.iter().zip(indices) {
                match domain {
                    Domain::Words(values) => {
                        let value = values[*index];
                        words.push(value);
                        arguments.push((*name, Arg::Word(Some(i64::from(value)))));
                        label.push_str(&format!("-{value:x}"));
                    }
                    Domain::Addresses(values) => {
                        let bytes = values[*index];
                        words.push(INPUT);
                        memory.push(known(INPUT, bytes.len() as u32, &bytes)?);
                        arguments.push((*name, Buffer::new(INPUT, bytes).into()));
                        label.push_str(&format!("-{bytes:02x?}"));
                    }
                }
            }
            for fill in LEAF_FILLS {
                let mut vendor = direct(
                    self.roots[leaf.vendor],
                    &words,
                    memory.clone(),
                    vec![radio_aperture(fill)],
                    vec![],
                );
                if let Some(callee) = leaf.prefix_until {
                    vendor.goal = ExecutionGoal::ObserveCall {
                        target: ExecutionSymbol {
                            source: self.vendor.source.clone(),
                            symbol: image_symbol_id(
                                &self.session.run.join("image/image.elf"),
                                &self.image_object,
                                callee,
                            )?,
                        },
                        include_tail: false,
                    };
                }
                let mut production = self.session.probes.invoke(
                    leaf.probe,
                    arguments.clone(),
                    vec![radio_aperture(fill)],
                    vec![],
                )?;
                production.arguments.resize(8, Some(0));
                let mut row = case(
                    format!("{}{label}-{fill:02x}", leaf.vendor),
                    vendor,
                    Some(production),
                    SessionReset::Cold,
                    false,
                );
                row.stack_fill = Some(fill);
                let relation = row.relation.as_mut().unwrap();
                relation.returns.low = leaf.returns;
                relation.effects = effects.clone();
                rows.push(row);
            }
        }
        Ok(rows)
    }
}

/// Compare every leaf; each must MATCH in every case and record effects.
pub fn exercise(ctx: &mut Mac) -> Result<()> {
    for leaf in LEAVES {
        let rows = ctx.cases(leaf)?;
        let count = rows.len() as u32;
        let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
        let records = ctx
            .submit(
                leaf.probe,
                &request(&vendor, Some(&production), None, rows, LEAF_EVENTS),
                Some(ComparisonVerdict::Match),
            )?
            .records
            .clone();
        for case in 0..count {
            for side in [false, true] {
                if !crate::i2c::all_complete(&records, case, side) {
                    return Err(invalid(format!(
                        "{} case {case} did not complete",
                        leaf.vendor
                    )));
                }
            }
            if crate::evidence::phy_effects(&crate::evidence::events(&records, case, false))
                .is_empty()
            {
                return Err(invalid(format!(
                    "{} case {case} has no register effect",
                    leaf.vendor
                )));
            }
        }
    }
    Ok(())
}

/// Evidence claims: every leaf with its production probe.
pub fn claims() -> Vec<(&'static str, &'static str, &'static str)> {
    LEAVES
        .iter()
        .map(|l| ("archive", l.vendor, l.probe))
        .collect()
}
