//! Wi-Fi MAC HAL leaves of the pinned `libpp.a` against compiled production.
//!
//! Each leaf is one vendor function and the production probe that runs its
//! HAL counterpart. Every case runs both over the whole radio register block
//! retained from one fill pattern, so every register bit the leaf reads takes
//! both values across the fills, and compares every register effect exactly
//! and, where the leaf returns one, the return word.
use crate::harness::{Arg, Buffer, Input, Result, case, direct, filled, invalid, known, selection};
use crate::layout::{INPUT, OUTPUT, radio_aperture};
use crate::phy::{image_layout, select};
use crate::session::{Session, image_symbol_id, request};
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallOutput, CallOutputScope, CallRepetition,
    CallResponse, ComparisonVerdict, EffectContractRef, EffectDisposition, EffectPattern,
    EffectRule, EffectSelector, EffectValue, ExecutionCase, ExecutionGoal, ExecutionSymbol,
    ExecutionTarget, LinkRequest, ObjectId, ObjectLocation, RegionLifetime, SessionReset,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// SHA-256 of the pinned `libpp.a`.
pub const LIBPP_SHA: &str = "f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e";
/// Names no pinned input defines: newlib `putchar`, which only the
/// `libpp.a` diagnostic dumps reachable from the transmit leaves call. They
/// resolve to an unmapped address, so reaching one stops the case.
const ABSENT: &[&str] = &["putchar"];
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

/// Values one parameter takes: ABI words, six-byte addresses the leaf reads
/// through a pointer to a buffer at `INPUT`, or the address of an output
/// object in the region at `OUTPUT`, filled with `OUTPUT_FILL` and compared
/// after the leaf; a nullable output also takes the null pointer.
#[derive(Clone, Copy)]
pub enum Domain {
    Words(&'static [u32]),
    Addresses(&'static [[u8; 6]]),
    Output {
        offset: u32,
        length: u32,
        nullable: bool,
    },
}

impl Domain {
    fn len(self) -> usize {
        match self {
            Self::Words(values) => values.len(),
            Self::Addresses(values) => values.len(),
            Self::Output { nullable, .. } => 1 + usize::from(nullable),
        }
    }
}

/// A non-null output object of `length` bytes at the start of the region.
const fn output(length: u32) -> Domain {
    Domain::Output {
        offset: 0,
        length,
        nullable: false,
    }
}

/// Initial bytes of an output object: bytes a leaf leaves alone compare too.
const OUTPUT_FILL: u8 = 0xa5;

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
    /// Builds both sides' objects from the semantic probe words when the
    /// vendor reads its arguments from its own objects.
    pub vendor_abi: Option<VendorAbi>,
    /// The vendor function is a ROM symbol rather than a `libpp.a` root.
    pub rom: bool,
}

/// Objects of one case: vendor argument words, initialized vendor and
/// production objects as (address, bytes), and vendor call models.
#[derive(Default)]
pub struct Objects {
    pub vendor_words: Vec<u32>,
    pub vendor: Vec<(u32, Vec<u8>)>,
    pub production: Vec<(u32, Vec<u8>)>,
    pub calls: Vec<CallDeclaration>,
}

/// Builds a case's objects from the semantic probe words; `rom` resolves a
/// ROM data symbol's address.
pub type VendorAbi = fn(&[u32], &dyn Fn(&str) -> Result<u32>) -> Result<Objects>;

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
        vendor_abi: None,
        rom: false,
    }
}

/// A leaf whose vendor function is the ROM symbol of that name.
const fn rom(leaf: Leaf) -> Leaf {
    Leaf { rom: true, ..leaf }
}

/// A leaf whose production counterpart adds `fences` ordering fences.
const fn ordered(leaf: Leaf, fences: u32) -> Leaf {
    Leaf {
        ordering_fences: fences,
        ..leaf
    }
}

/// A leaf whose vendor reads its semantic arguments from objects `abi` builds.
const fn objects(leaf: Leaf, abi: VendorAbi) -> Leaf {
    Leaf {
        vendor_abi: Some(abi),
        ..leaf
    }
}

/// `hal_mac_tx_config_edca` object at `INPUT`: a context pointer, then the
/// queue byte, the AIFSN byte and the contention-window halfword. The
/// context holds at 0x34 a pointer to the interface record, whose word at
/// 0x10 carries the interface in bits 18 and 19. The probe's first word is
/// the object address itself.
fn edca_abi(words: &[u32], _rom: &dyn Fn(&str) -> Result<u32>) -> Result<Objects> {
    const CONTEXT: u32 = INPUT + 0x100;
    const INTERFACE_RECORD: u32 = INPUT + 0x200;
    let [_, queue, aifsn, window, interface] = words else {
        unreachable!("EDCA words: object, queue, AIFSN, window, interface")
    };
    let mut object = CONTEXT.to_le_bytes().to_vec();
    object.extend([*queue as u8, *aifsn as u8]);
    object.extend((*window as u16).to_le_bytes());
    let mut context = vec![0u8; 0x38];
    context[0x34..].copy_from_slice(&INTERFACE_RECORD.to_le_bytes());
    let mut record = vec![0u8; 0x14];
    record[0x10..].copy_from_slice(&(interface << 18).to_le_bytes());
    Ok(Objects {
        vendor_words: vec![INPUT],
        vendor: vec![
            (INPUT, object),
            (CONTEXT, context),
            (INTERFACE_RECORD, record),
        ],
        ..Default::default()
    })
}

/// Canonical HT transmit parameters of the reviewed `hal_mac_tx_set_ppdu`
/// fixture, in `CanonicalHtTxParameters` order: queue 0, descriptor head,
/// MCS 7, short guard interval, 40 MHz, A-MPDU, length 0xc2e, two
/// descriptors, data powers 1/1, RTS powers 2/2, spacing density 5, no
/// timeout, scheduler and packet priority 1, one priority, zero AIFSN and
/// window, the station interface, no hardware key and no TXOP.
const PPDU_CANONICAL: [u32; 22] = [
    0,
    PPDU_DESCRIPTOR,
    7,
    1,
    1,
    1,
    0xc2e,
    2,
    1,
    1,
    2,
    2,
    1,
    0,
    1,
    1,
    1,
    0,
    0,
    0,
    0,
    0,
];
/// Vendor PP object addresses of that fixture: the transmit context, the
/// first descriptor, the `pTxRx` rate table and the OSI function table.
const PPDU_PROGRAM: u32 = 0x3fff_1000;
const PPDU_DESCRIPTOR: u32 = 0x3fff_1200;
const PPDU_AUXILIARY: u32 = 0x3fff_1600;
const PPDU_OSI_TABLE: u32 = 0x3fff_1800;
/// Bytes of the OSI function table through the `coex_pti_clamp` slot.
const PPDU_OSI_BYTES: usize = 0x1ac;
const PPDU_COEX_PTI_CLAMP_SLOT: usize = 0x1a8;
/// Unmapped address the OSI slot names; a call model answers it.
const PPDU_COEX_PTI_CLAMP: u32 = 0x5000_0000;
/// Vendor PP objects of the fixture, as (address, words).
const PPDU_VENDOR: &[(u32, &[u32])] = &[
    (PPDU_PROGRAM, &[0x3fff_1100, 0]),
    (
        0x3fff_1100,
        &[
            0,
            PPDU_DESCRIPTOR,
            0,
            0,
            0,
            // Two halfword lengths summed by the bounded HT A-MPDU branch
            // of mac_tx_set_len; the second remains zero.
            0x0000_0c2e,
            0,
            0,
            0,
            0,
            0,
            0x3fff_1500,
            0,
            0x3fff_1300,
        ],
    ),
    (PPDU_DESCRIPTOR, &[0x3fff_1400, 0x0000_8000, 0, 0]),
    (
        0x3fff_1300,
        &[
            // Word-zero bit 14 enables the guarded channel-width branch;
            // word-two bit 15 selects 40 MHz in it and in mac_tx_set_htsig.
            0x004c_6009,
            0,
            0x0000_8000,
            0x0000_0021,
            0,
            0,
            0,
            0,
            0x0001_0001,
            0,
            0x0002_0000,
            0x0002_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    (0x3fff_1400, &[0]),
    (0x3fff_1500, &[0]),
    (0x3fff_1580, &[0x0028_0000, 0x0004_0000, 0, 0, 0, 0]),
    // The selected pTxRx rate row contributes entry class one to both packed
    // length words at byte offsets 0x40 and 0x41.
    (PPDU_AUXILIARY, &[0; 16]),
    (PPDU_AUXILIARY + 0x40, &[0x0000_0101, 0x0000_0c2e, 0]),
];
/// ROM `s_phy_get_max_pwr` rows the fixture seeds.
const PPDU_MAX_POWER: [u32; 22] = [
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0202_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0101_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0201_0201,
    0x0000_0201,
];

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The reviewed ordinary-queue-zero HT40 MCS7 A-MPDU fixture of
/// `hal_mac_tx_set_ppdu`: vendor PP objects, the ROM rate-table pointer,
/// power rows and OSI table, and the canonical production parameters.
fn ppdu_abi(_words: &[u32], rom: &dyn Fn(&str) -> Result<u32>) -> Result<Objects> {
    let mut vendor: Vec<(u32, Vec<u8>)> = PPDU_VENDOR
        .iter()
        .map(|(address, values)| (*address, words(values)))
        .collect();
    let mut table = vec![0u8; PPDU_OSI_BYTES];
    table[PPDU_COEX_PTI_CLAMP_SLOT..].copy_from_slice(&PPDU_COEX_PTI_CLAMP.to_le_bytes());
    vendor.extend([
        (PPDU_OSI_TABLE, table),
        (rom("pTxRx")?, PPDU_AUXILIARY.to_le_bytes().to_vec()),
        (rom("g_osi_funcs_p")?, PPDU_OSI_TABLE.to_le_bytes().to_vec()),
        (rom("s_phy_get_max_pwr")?, words(&PPDU_MAX_POWER)),
    ]);
    let clamp = CallDeclaration {
        id: "coex-pti-clamp".into(),
        applicability: "the OSI coexistence PTI clamp returns one through its output word".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: PPDU_COEX_PTI_CLAMP,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 2,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            // The clamp stores its one-byte result through its second
            // argument, a byte of the caller's frame.
            outputs: vec![CallOutput {
                pointer_argument: 1,
                byte_offset: 0,
                width: 1,
                value: 1,
                scope: CallOutputScope::PrivateStack,
            }],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Finite,
    };
    Ok(Objects {
        vendor_words: vec![PPDU_PROGRAM, PPDU_AUXILIARY],
        vendor,
        production: vec![(INPUT, words(&PPDU_CANONICAL))],
        calls: vec![clamp],
    })
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

/// AIFSN values: zero, a default, the four-bit maximum and one bit beyond.
const AIFSN: &[u32] = &[0, 2, 15, 0x1f];
/// Contention windows: zero, a default, the ten-bit maximum and one beyond.
const WINDOWS: &[u32] = &[0, 0x15, 0x3ff, 0x7ff];
/// The EDCA object address the probe receives and ignores.
const EDCA_OBJECT: &[u32] = &[INPUT];

/// STA TSF low and high words, each through a nullable output pointer.
const TSF_LOW: Domain = Domain::Output {
    offset: 0,
    length: 4,
    nullable: true,
};
const TSF_HIGH: Domain = Domain::Output {
    offset: 4,
    length: 4,
    nullable: true,
};

/// Bytes of the transmit Block Ack record: control, sequence and bitmap.
const BLOCK_ACK_BYTES: u32 = 12;

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
    leaf(
        "hal_mac_tx_get_blockack",
        "open_libpp_tx_trace_hal_mac_tx_get_blockack",
        &[
            ("queue", Domain::Words(QUEUES)),
            ("output_address", output(BLOCK_ACK_BYTES)),
        ],
        true,
    ),
    objects(
        leaf(
            "hal_mac_tx_config_edca",
            "open_libpp_tx_trace_hal_mac_tx_config_edca",
            &[
                ("_vendor_config_address", Domain::Words(EDCA_OBJECT)),
                ("queue", Domain::Words(QUEUES)),
                ("aifsn", Domain::Words(AIFSN)),
                ("contention_window", Domain::Words(WINDOWS)),
                ("interface", Domain::Words(INTERFACES)),
            ],
            true,
        ),
        edca_abi,
    ),
    rom(leaf(
        "hal_get_sta_tsf",
        "open_rom_power_tsf_trace_hal_get_sta_tsf",
        &[("low", TSF_LOW), ("high", TSF_HIGH)],
        false,
    )),
    objects(
        leaf(
            "hal_mac_tx_set_ppdu",
            "open_libpp_tx_trace_hal_mac_tx_set_ppdu",
            &[
                ("program_address", Domain::Words(&[INPUT])),
                ("_vendor_auxiliary", Domain::Words(&[PPDU_AUXILIARY])),
            ],
            false,
        ),
        ppdu_abi,
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
            .filter(|l| !l.rom)
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
            absent: ABSENT.iter().map(|n| (*n).to_owned()).collect(),
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

    /// Entry address of the vendor function of `leaf`.
    fn vendor_entry(&self, leaf: &Leaf) -> Result<u32> {
        if leaf.rom {
            Ok(u32::try_from(
                crate::harness::symbol(
                    &self.session.inventory,
                    crate::layout::ROM_INPUT as usize,
                    leaf.vendor,
                )?
                .value,
            )?)
        } else {
            Ok(self.roots[leaf.vendor])
        }
    }

    /// Exact code endpoint of the vendor function of `leaf`.
    fn vendor_endpoint(&self, leaf: &Leaf) -> Result<blobray_domain::CallEndpoint> {
        if leaf.rom {
            self.session
                .input_endpoint(crate::layout::ROM_INPUT, leaf.vendor)
        } else {
            self.session.image_endpoint(
                &self.vendor,
                &self.image_object,
                leaf.vendor,
                self.roots[leaf.vendor],
            )
        }
    }

    /// The reviewed contract of a leaf whose production adds ordering
    /// fences: exactly that many full fences, every other effect compared
    /// exactly.
    fn ordering_contract(&mut self, leaf: &Leaf) -> Result<EffectContractRef> {
        let vendor = self.vendor_endpoint(leaf)?;
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
            // End of the output region the parameters address.
            let mut output: Option<u32> = None;
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
                    Domain::Output {
                        offset,
                        length,
                        nullable,
                    } => {
                        let address = if *nullable && *index == 0 {
                            0
                        } else {
                            OUTPUT + offset
                        };
                        words.push(address);
                        arguments.push((*name, Arg::Word(Some(i64::from(address)))));
                        output = Some(output.unwrap_or(0).max(offset + length));
                        label.push_str(&format!("-{address:x}"));
                    }
                }
            }
            let (output_memory, observe) = match output {
                Some(length) => (
                    vec![filled(OUTPUT, length, OUTPUT_FILL)?],
                    vec![selection(OUTPUT, length)],
                ),
                None => (vec![], vec![]),
            };
            let regions = |objects: &[(u32, Vec<u8>)]| {
                objects
                    .iter()
                    .map(|(address, bytes)| known(*address, bytes.len() as u32, bytes))
                    .collect::<Result<Vec<_>>>()
            };
            let rom = |name: &str| -> Result<u32> {
                Ok(u32::try_from(
                    crate::harness::symbol(
                        &self.session.inventory,
                        crate::layout::ROM_INPUT as usize,
                        name,
                    )?
                    .value,
                )?)
            };
            let (vendor_words, mut vendor_memory, production_memory, vendor_calls) =
                match leaf.vendor_abi {
                    Some(abi) => {
                        let objects = abi(&words, &rom)?;
                        (
                            objects.vendor_words,
                            regions(&objects.vendor)?,
                            regions(&objects.production)?,
                            objects.calls,
                        )
                    }
                    None => (words.clone(), memory.clone(), vec![], vec![]),
                };
            vendor_memory.extend(output_memory.clone());
            for fill in LEAF_FILLS {
                let mut vendor = direct(
                    self.vendor_entry(leaf)?,
                    &vendor_words,
                    vendor_memory.clone(),
                    vec![radio_aperture(fill)],
                    observe.clone(),
                );
                vendor.calls = vendor_calls.clone();
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
                    observe.clone(),
                )?;
                production.memory.extend(output_memory.clone());
                production.memory.extend(production_memory.clone());
                production.arguments.resize(8, Some(0));
                let mut row = case(
                    format!("{}{label}-{fill:02x}", leaf.vendor),
                    vendor,
                    Some(production),
                    SessionReset::Cold,
                    output.is_some(),
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
        .map(|l| (if l.rom { "rom" } else { "archive" }, l.vendor, l.probe))
        .collect()
}
