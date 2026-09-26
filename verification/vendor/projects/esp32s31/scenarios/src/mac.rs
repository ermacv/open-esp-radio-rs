//! Wi-Fi MAC HAL leaves of the pinned `libpp.a` against compiled production.
//!
//! Each leaf is one vendor function and the production probe that runs its
//! HAL counterpart. Every case runs both over the whole radio register block
//! retained from one fill pattern, so every register bit the leaf reads takes
//! both values across the fills, and compares every register effect exactly
//! and, where the leaf returns one, the return word.
use crate::harness::{Arg, Input, Result, case, direct, invalid};
use crate::layout::radio_aperture;
use crate::phy::{image_layout, select};
use crate::session::{Session, request};
use blobray_domain::{
    ComparisonVerdict, EffectContractRef, EffectDisposition, EffectPattern, EffectRule,
    EffectSelector, EffectValue, ExecutionCase, ExecutionTarget, LinkRequest, ObjectId,
    ObjectLocation, SessionReset,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// SHA-256 of the pinned `libpp.a`.
pub const LIBPP_SHA: &str = "f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e";
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
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: crate::harness::Budget,
    pub patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// One vendor leaf, its production probe and the argument domain of each
/// parameter, in ABI order; the cases are their product.
pub struct Leaf {
    pub vendor: &'static str,
    pub probe: &'static str,
    pub parameters: &'static [(&'static str, &'static [u32])],
    pub returns: bool,
    /// Production follows the leaf's last register write with one full
    /// fence, ordering it before later device accesses.
    pub ordering_fence: bool,
}

const fn leaf(
    vendor: &'static str,
    probe: &'static str,
    parameters: &'static [(&'static str, &'static [u32])],
    returns: bool,
) -> Leaf {
    Leaf {
        vendor,
        probe,
        parameters,
        returns,
        ordering_fence: false,
    }
}

/// A leaf whose production counterpart adds the ordering fence.
const fn ordered(leaf: Leaf) -> Leaf {
    Leaf {
        ordering_fence: true,
        ..leaf
    }
}

/// Full-fence predecessor and successor sets: device input, output, memory
/// reads and writes.
const FULL_FENCE: u8 = 0xf;

/// Every compared leaf.
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
        &[("selector", TSF_FRESH_EPOCH)],
        false,
    ),
    leaf(
        "hal_mac_interrupt_get_event",
        "open_libpp_trace_hal_mac_interrupt_ret_get_event",
        &[],
        true,
    ),
    ordered(leaf(
        "hal_mac_interrupt_clr_event",
        "open_libpp_trace_hal_mac_interrupt_ret_clr_event",
        &[("events", EVENT_MASKS)],
        false,
    )),
    leaf(
        "hal_pwr_interrupt_get_event",
        "open_libpp_power_irq_trace_hal_pwr_interrupt_get_event",
        &[],
        true,
    ),
    ordered(leaf(
        "hal_pwr_interrupt_clr_event",
        "open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event",
        &[("events", EVENT_MASKS)],
        false,
    )),
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
        &[("address", RX_BASES)],
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
        &[("value", CCA)],
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
        &[("queue", QUEUES)],
        true,
    ),
    leaf(
        "hal_mac_is_txq_valid",
        "open_libpp_tx_trace_hal_mac_is_txq_valid",
        &[("queue", QUEUES)],
        true,
    ),
    leaf(
        "hal_mac_set_txq_invalid",
        "open_libpp_tx_trace_hal_mac_set_txq_invalid",
        &[("queue", QUEUES)],
        false,
    ),
    leaf(
        "hal_mac_txq_disable",
        "open_libpp_tx_trace_hal_mac_txq_disable",
        &[("queue", QUEUES)],
        false,
    ),
    leaf(
        "hal_mac_txq_disable",
        "open_ordinary_tx_ownership_disable",
        &[("queue", QUEUES)],
        false,
    ),
    leaf(
        "hal_mac_clr_txq_state",
        "open_ordinary_tx_ownership_acknowledge",
        &[("selector", COMPLETION_CLEAR), ("queue", QUEUES)],
        false,
    ),
    leaf(
        "hal_disable_sta_beacon_filter",
        "open_wifi_sta_trace_hal_disable_sta_beacon_filter",
        &[],
        false,
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
            &[crate::layout::ROM_INPUT],
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

    /// The reviewed contract of a leaf whose production adds the ordering
    /// fence: exactly one full fence, every other effect compared exactly.
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
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "production orders the event clear before later device accesses; the vendor \
                leaves ordering to its caller"
                .into(),
        };
        let applicability = "one MAC event clear over retained radio registers";
        self.session.review_effects(
            &format!("{}-effects", leaf.probe),
            &format!("esp32s31.mac.{}.effects", leaf.vendor),
            crate::contracts::phy_contract(vendor, production, vec![rule], applicability),
            "every register effect compares exactly; production adds one ordering fence",
        )
    }

    /// Both sides of `leaf` for every argument combination and fill.
    fn cases(&mut self, leaf: &Leaf) -> Result<Vec<ExecutionCase>> {
        let effects = if leaf.ordering_fence {
            Some(self.ordering_contract(leaf)?)
        } else {
            None
        };
        let mut combinations: Vec<Vec<u32>> = vec![vec![]];
        for (_, domain) in leaf.parameters {
            combinations = combinations
                .into_iter()
                .flat_map(|prefix| {
                    domain.iter().map(move |value| {
                        let mut words = prefix.clone();
                        words.push(*value);
                        words
                    })
                })
                .collect();
        }
        let mut rows = vec![];
        for words in &combinations {
            for fill in LEAF_FILLS {
                let vendor = direct(
                    self.roots[leaf.vendor],
                    words,
                    vec![],
                    vec![radio_aperture(fill)],
                    vec![],
                );
                let arguments = leaf
                    .parameters
                    .iter()
                    .zip(words)
                    .map(|((name, _), word)| (*name, Arg::Word(Some(i64::from(*word)))))
                    .collect();
                let mut production = self.session.probes.invoke(
                    leaf.probe,
                    arguments,
                    vec![radio_aperture(fill)],
                    vec![],
                )?;
                production.arguments.resize(8, Some(0));
                let mut row = case(
                    format!("{}-{words:x?}-{fill:02x}", leaf.vendor),
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
