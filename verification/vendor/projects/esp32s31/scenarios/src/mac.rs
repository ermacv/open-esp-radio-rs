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
    CallBinding, CallBoundary, CallCapture, CallDeclaration, CallOutput, CallOutputScope,
    CallRepetition, CallResponse, ComparisonVerdict, EffectContractRef, EffectDisposition,
    EffectPattern, EffectRule, EffectSelector, EffectValue, ExecutionCase, ExecutionGoal,
    ExecutionSymbol, ExecutionTarget, LinkRequest, MemoryPair, ObjectId, ObjectLocation,
    RegionLifetime, SessionReset,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Names no pinned input defines: newlib `putchar`, which only the
/// `libpp.a` diagnostic dumps reachable from the transmit leaves call. They
/// resolve to an unmapped address, so reaching one stops the case.
const ABSENT: &[&str] = &["putchar"];
/// Input index of the compiled production probe ELF.
const PROBE_INPUT: usize = 2;
/// Input index of the vendor Wi-Fi firmware.
const PHY_SDK_INPUT: u64 = 3;
/// Input index of the pinned `libnet80211.a`.
const NET80211_INPUT: u64 = 4;
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
    /// Pinned `libnet80211.a`, whose roots call into `libpp.a`.
    pub libnet80211: PathBuf,
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
/// Source of the bytes a setup phase copies into a linked-image object.
const IMAGE_SOURCE: u32 = 0x3fff_b000;

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
    /// The vendor function is a `libnet80211.a` root linked with `libpp.a`.
    pub net80211: bool,
    /// Object states the builder receives after the probe words: a further
    /// case dimension that reaches the probe only through the objects.
    pub states: &'static [u32],
    /// A dispatcher compared up to its selected callee: for the case words,
    /// the vendor callee and the production callee both sides stop before.
    /// Reaching another callee leaves the goal unmet; the first argument
    /// word at both stops must be the case's first word.
    pub dispatch: Option<Dispatch>,
    /// Vendor functions answered with a zero return and no other effect:
    /// assertions the compared domain never fails.
    pub quiet_calls: &'static [&'static str],
}

/// The vendor and production callees a dispatcher selects for the words.
pub type Dispatch = fn(&[u32]) -> (&'static str, &'static str);

/// Objects of one case: vendor argument words, initialized vendor and
/// production objects as (address, bytes), and vendor call models.
#[derive(Default)]
pub struct Objects {
    pub vendor_words: Vec<u32>,
    pub vendor: Vec<(u32, Vec<u8>)>,
    pub production: Vec<(u32, Vec<u8>)>,
    pub calls: Vec<CallDeclaration>,
    /// Object bytes both sides share and the relation compares after the
    /// leaf, as (address, length); each side receives the same objects.
    pub compared: Vec<(u32, u32)>,
    /// Vendor objects inside the linked image's data, as (address, bytes):
    /// setup phases copy them in with the captured ROM `memcpy`, and the
    /// compared phase keeps them warm.
    pub image: Vec<(u32, Vec<u8>)>,
}

/// Builds a case's objects from the semantic probe words, followed by the
/// object state when the leaf has states.
pub type VendorAbi = fn(&[u32], &Vendor<'_>) -> Result<Objects>;

/// What a builder may read from the vendor side: linked-image or ROM symbol
/// addresses and data sections of the captured archive.
pub struct Vendor<'a> {
    pub resolve: &'a dyn Fn(&str) -> Result<u32>,
    /// Symbols the linked image defines; any other symbol is a ROM address.
    pub image: &'a BTreeMap<String, u32>,
    pub rates: &'a RateTables,
}

impl Vendor<'_> {
    fn symbol(&self, name: &str) -> Result<u32> {
        (self.resolve)(name)
    }

    /// The boundary of a call model answering `name`: captured code when the
    /// linked image defines it, an unmapped address otherwise.
    fn boundary(&self, name: &str) -> CallBoundary {
        call_boundary(self.image, name)
    }
}

fn call_boundary(image: &BTreeMap<String, u32>, name: &str) -> CallBoundary {
    if image.contains_key(name) {
        CallBoundary::CapturedCode
    } else {
        CallBoundary::Unmapped
    }
}

/// `libpp.a[trc.o]` 802.11g retry data: the rate-code-to-record index table
/// `rc11GRate2SchedIdx` reads, and the schedule arena `rc11GSchedTbl`.
pub struct RateTables {
    pub index: Vec<u8>,
    pub arena: Vec<u8>,
}

impl RateTables {
    /// Object and data sections of the tables.
    const OBJECT: &'static str = "trc.o";
    const INDEX_SECTION: &'static str = ".rodata.CSWTCH.73";
    const ARENA_SECTION: &'static str = ".data.rc11GSchedTbl";
    /// Bytes of one schedule record.
    const RECORD: usize = 12;
    /// Index-table entry of a rate with no 802.11g record.
    const UNMAPPED: u8 = 0xff;

    /// The vendor schedule record of legacy rate `code`.
    fn record(&self, code: u32) -> Result<Vec<u8>> {
        let index = *self
            .index
            .get(code as usize)
            .ok_or_else(|| invalid(format!("rate {code:#x} outside the vendor index table")))?;
        if index == Self::UNMAPPED {
            return Err(invalid(format!(
                "rate {code:#x} has no vendor 802.11g record"
            )));
        }
        let start = usize::from(index) * Self::RECORD;
        self.arena
            .get(start..start + Self::RECORD)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| invalid(format!("vendor record {index} outside the arena")))
    }

    /// Whether the attempt counts of `code`'s record reach its publication
    /// limit at byte 0x08, so the retry-limit owner ends the MPDU before the
    /// record is exhausted.
    fn covers_publication_limit(&self, code: u32) -> Result<bool> {
        let record = self.record(code)?;
        let attempts: u32 = (0..4).map(|pair| u32::from(record[2 * pair + 1])).sum();
        Ok(attempts >= u32::from(record[8]))
    }

    /// The rate codes the vendor maps to an 802.11g record.
    fn mapped(&self) -> Vec<u32> {
        (0..self.index.len() as u32)
            .filter(|code| self.index[*code as usize] != Self::UNMAPPED)
            .collect()
    }
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
        vendor_abi: None,
        rom: false,
        net80211: false,
        states: &[],
        dispatch: None,
        quiet_calls: &[],
    }
}

/// A dispatcher leaf compared up to the callee `select` names.
const fn dispatching(leaf: Leaf, select: Dispatch) -> Leaf {
    Leaf {
        dispatch: Some(select),
        ..leaf
    }
}

/// A leaf whose vendor `calls` return zero with no other effect.
const fn quiet(leaf: Leaf, calls: &'static [&'static str]) -> Leaf {
    Leaf {
        quiet_calls: calls,
        ..leaf
    }
}

/// A leaf whose vendor function is the ROM symbol of that name.
const fn rom(leaf: Leaf) -> Leaf {
    Leaf { rom: true, ..leaf }
}

/// A leaf whose vendor function is the `libnet80211.a` root of that name.
const fn net80211(leaf: Leaf) -> Leaf {
    Leaf {
        net80211: true,
        ..leaf
    }
}

/// A leaf whose production counterpart adds `fences` ordering fences.
const fn ordered(leaf: Leaf, fences: u32) -> Leaf {
    Leaf {
        ordering_fences: fences,
        ..leaf
    }
}

/// A leaf whose objects also take each of `states`.
const fn stated(leaf: Leaf, states: &'static [u32]) -> Leaf {
    Leaf { states, ..leaf }
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
fn edca_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
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
fn ppdu_abi(_words: &[u32], vendor_side: &Vendor<'_>) -> Result<Objects> {
    let rom = |name: &str| vendor_side.symbol(name);
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
        compared: vec![],
        ..Default::default()
    })
}

/// `rcGetRate` fixture of the normal 802.11g schedules: the rate context
/// at `RATE_CONTEXT`, whose schedule flags at 0x0c select no fixed rate, and
/// the transmit descriptor at `RATE_DESCRIPTOR`, whose word 1 carries the
/// MPDU, short and long retry counters in bytes 1..3, whose byte 0x0c
/// receives the selected rate and whose word at 0x1c points to the vendor
/// schedule record of the initial rate, taken from the captured arena.
const RATE_CONTEXT: u32 = 0x3fff_1100;
const RATE_DESCRIPTOR: u32 = 0x3fff_1000;
const RATE_SCHEDULE: u32 = 0x3fff_1200;
/// Legacy initial rates: every code the vendor maps to an 802.11g record,
/// checked against the captured index table when the scenario starts.
const RATE_CODES: &[u32] = &[0, 1, 2, 5, 6, 8, 9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf];
/// Long-range codes the vendor also maps to 802.11g records. Production
/// publishes no long-range frame, so its ordinary retry selector admits no
/// long-range initial rate.
const UNADMITTED_RATE_CODES: &[u32] = &[0x29, 0x2a];
/// Retry-counter words: initial publication, second publication, first-rate
/// fallback, a short counter dominating the MPDU counter and a count past
/// the third pair of every record.
const RATE_COUNTERS: &[u32] = &[0, 0x0001_0100, 0x0002_0200, 0x0004_0200, 0x0000_0700];
/// Descriptor byte that receives the selected rate.
const RATE_SELECTED: u32 = RATE_DESCRIPTOR + 0x0c;

fn rate_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let resolve = |name: &str| vendor.symbol(name);
    let [_, _, rate, counters] = words else {
        unreachable!("rate words: context, descriptor, initial rate, counter state")
    };
    // Through word 0x30, whose format flags the tail after `rcGetSMPDURate`
    // reads; clear flags select the legacy path.
    let mut descriptor = [0u32; 13];
    descriptor[1] = *counters;
    // The selected-rate byte starts all set, so both sides must write it.
    descriptor[3] = u32::MAX;
    descriptor[7] = RATE_SCHEDULE;
    let objects = vec![
        (RATE_DESCRIPTOR, self::words(&descriptor)),
        (RATE_CONTEXT, vec![0; 16]),
        (RATE_SCHEDULE, vendor.rates.record(*rate)?),
    ];
    let assert = CallDeclaration {
        id: "wifi-assert".into(),
        applicability: "a vendor assertion the reviewed schedule never fails".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: resolve("wifi_assert")?,
            boundary: vendor.boundary("wifi_assert"),
            allow_tail: false,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    };
    Ok(Objects {
        vendor_words: vec![RATE_CONTEXT, RATE_DESCRIPTOR],
        vendor: objects.clone(),
        production: objects,
        calls: vec![assert],
        compared: vec![(RATE_SELECTED, 1)],
        ..Default::default()
    })
}

/// Initial bytes of each compared range, from the vendor objects covering it.
fn compared_bytes(objects: &[(u32, Vec<u8>)], compared: &[(u32, u32)]) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    for (address, length) in compared {
        let (start, object) = objects
            .iter()
            .find(|(start, object)| {
                *start <= *address && address + length <= start + object.len() as u32
            })
            .ok_or_else(|| invalid(format!("compared range {address:#x} has no object")))?;
        let offset = (address - start) as usize;
        bytes.extend_from_slice(&object[offset..offset + *length as usize]);
    }
    Ok(bytes)
}

/// Addresses of the named symbols of the linked image, including the
/// absolute companion definitions.
fn image_symbols(elf: &std::path::Path) -> Result<BTreeMap<String, u32>> {
    use object::{Object, ObjectSymbol};
    let bytes = std::fs::read(elf)?;
    let file = object::File::parse(&*bytes)?;
    let mut symbols = BTreeMap::new();
    for symbol in file.symbols() {
        if let (Ok(name), Ok(address)) = (symbol.name(), u32::try_from(symbol.address()))
            && !name.is_empty()
            && !symbol.is_undefined()
        {
            symbols.entry(name.to_owned()).or_insert(address);
        }
    }
    Ok(symbols)
}

/// Transmit-error details of status four and the retry leaf each selects.
const TX_ERROR_DETAILS: &[u32] = &[0, 1, 2, 3, 4, 5, 6];
/// Ordinary queues the dispatcher cases use: the lowest and the highest.
const TX_ERROR_QUEUES: &[u32] = &[0, 4];

/// `lmacProcessTxError` of status four: detail zero is a CTS timeout,
/// details two and six ACK timeouts, and the others collisions.
fn tx_error_leaf(words: &[u32]) -> (&'static str, &'static str) {
    match words[1] {
        0 => ("lmacProcessCtsTimeout", "open_libpp_tx_retry_cts_timeout"),
        2 | 6 => ("lmacProcessAckTimeout", "open_libpp_tx_retry_ack_timeout"),
        _ => ("lmacProcessCollision", "open_libpp_tx_retry_collision"),
    }
}

/// `lmacProcessTxError` loads ROM `our_instances_ptr` before dispatching;
/// only the key-error detail `0xc0`, outside these cases, dereferences it.
fn tx_error_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let resolve = |name: &str| vendor.symbol(name);
    Ok(Objects {
        vendor_words: words.to_vec(),
        vendor: vec![(resolve("our_instances_ptr")?, vec![0; 4])],
        ..Default::default()
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

/// One case: its setup phases, the compared phase, the initial bytes of its
/// compared objects and its probe words.
type LeafCase = (Vec<ExecutionCase>, ExecutionCase, Vec<u8>, Vec<u32>);

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
    stated(
        objects(
            leaf(
                "rcGetRate",
                "open_libpp_tx_retry_trace_rc_get_rate",
                &[
                    ("_rate_context", Domain::Words(&[RATE_CONTEXT])),
                    ("descriptor_address", Domain::Words(&[RATE_DESCRIPTOR])),
                    ("initial_rate", Domain::Words(RATE_CODES)),
                ],
                false,
            ),
            rate_abi,
        ),
        RATE_COUNTERS,
    ),
    quiet(
        dispatching(
            objects(
                leaf(
                    "lmacProcessTxError",
                    "open_libpp_tx_retry_trace_lmac_process_tx_error",
                    &[
                        ("queue", Domain::Words(TX_ERROR_QUEUES)),
                        ("detail", Domain::Words(TX_ERROR_DETAILS)),
                        ("_selector", Domain::Words(&[0])),
                    ],
                    false,
                ),
                tx_error_abi,
            ),
            tx_error_leaf,
        ),
        &["wifi_assert"],
    ),
    net80211(objects(
        leaf(
            "wifi_set_rx_policy",
            "open_wifi_sta_ap_trace_wifi_set_rx_policy",
            &[
                ("policy", Domain::Words(RX_POLICIES)),
                ("address_low", Domain::Words(RX_ADDRESS_LOW)),
                ("address_high", Domain::Words(RX_ADDRESS_HIGH)),
                ("mode", Domain::Words(RX_MODES)),
            ],
            false,
        ),
        rx_policy_abi,
    )),
    // Production publishes the interface addresses in its cold transaction
    // and claims only the suffix of policy zero; the vendor address
    // republication is answered without effect.
    quiet(
        net80211(objects(
            leaf(
                "wifi_set_rx_policy",
                "open_wifi_sta_ap_trace_disable_all_role_receive",
                &[("_policy", Domain::Words(&[RX_POLICY_NONE]))],
                false,
            ),
            rx_disable_all_abi,
        )),
        &["ic_set_mac"],
    ),
];

/// Every byte of data section `section` of `object` in the captured archive.
fn vendor_section(session: &Session, object: &str, section: &str) -> Result<Vec<u8>> {
    let object = crate::harness::named_object(&session.inventory, 0, object)?;
    let record = crate::harness::named_section(object, section)?;
    let request = crate::harness::data_request(
        &session.revision,
        blobray_domain::FunctionSource::Input { input: 0 },
        object,
        blobray_domain::DataSelector::Section {
            section: record.index,
            offset: 0,
            length: record.size,
        },
    );
    let name = format!("section{}", section.replace('.', "-"));
    session.data(&name, &request, &session.run.join(&name))
}

/// `wifi_set_rx_policy` policies the production role-receive HAL admits:
/// station disabled, station, access point and access point disabled.
const RX_POLICIES: &[u32] = &[2, 6, 8, 9];
/// Low and high words of the policy address: all clear and a pattern.
const RX_ADDRESS_LOW: &[u32] = &[0, 0x3322_1100];
const RX_ADDRESS_HIGH: &[u32] = &[0, 0x5544];
/// `wifi_set_rx_policy` code that republishes both interface addresses and
/// disables both receive contexts.
const RX_POLICY_NONE: u32 = 0;
/// Station receive modes of policy six.
const RX_MODES: &[u32] = &[1, 2];
/// `g_ic` bytes the policy reads and writes, and its fields: the word
/// pointing to the station record, the station-mode word and the
/// access-point address.
const IC_BYTES: usize = 0x2d0;
const IC_STATION: usize = 0;
const IC_STATION_MODE: usize = 0x74;
const IC_AP_ADDRESS: usize = 0x214;
/// Station record word at 0x1460 points to the BSS record, whose bytes
/// 4..10 carry the BSSID.
const RX_STATION: u32 = 0x3fff_7000;
const RX_STATION_BSS: u32 = 0x1460;
const RX_BSS: u32 = 0x3fff_9000;

/// The policy's `g_ic` context and station records built from the probe
/// words: policy, address low and high words and the station mode.
fn rx_policy_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [policy, low, high, mode] = words else {
        unreachable!("receive-policy words: policy, address low, address high, mode")
    };
    let mut address = low.to_le_bytes().to_vec();
    address.extend(&high.to_le_bytes()[..2]);
    let mut context = vec![0u8; IC_BYTES];
    context[IC_STATION..IC_STATION + 4].copy_from_slice(&RX_STATION.to_le_bytes());
    context[IC_STATION_MODE..IC_STATION_MODE + 4]
        .copy_from_slice(&u32::from(*mode == 2).to_le_bytes());
    context[IC_AP_ADDRESS..IC_AP_ADDRESS + 6].copy_from_slice(&address);
    let mut bss = vec![0u8; 4];
    bss.extend(&address);
    Ok(Objects {
        vendor_words: vec![*policy],
        vendor: vec![
            (RX_STATION + RX_STATION_BSS, RX_BSS.to_le_bytes().to_vec()),
            (RX_BSS, bss),
        ],
        image: vec![(vendor.symbol("g_ic")?, context)],
        ..Default::default()
    })
}

/// The policy-zero `g_ic` context: no address and the first station mode.
fn rx_disable_all_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    rx_policy_abi(&[words[0], 0, 0, RX_MODES[0]], vendor)
}

/// Linked `libpp.a` image with its captured roots and both execution targets.
pub struct Mac {
    pub session: Session,
    pub rates: RateTables,
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
                sha256: Some(crate::artifacts::sha256("libpp")),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(crate::artifacts::sha256("rom")),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
            crate::phy::phy_sdk_input(&options.phy_sdk),
            Input {
                role: "libnet80211",
                path: &options.libnet80211,
                sha256: Some(crate::artifacts::sha256("libnet80211")),
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
        // The first leaf is the link entry; the others are further roots,
        // each selected in the archive that defines it.
        let mut vendors: Vec<(u64, &str)> = LEAVES
            .iter()
            .filter(|l| !l.rom)
            .map(|l| (if l.net80211 { NET80211_INPUT } else { 0 }, l.vendor))
            .filter(|(_, v)| *v != LEAVES[0].vendor)
            .collect();
        vendors.sort_unstable();
        vendors.dedup();
        let roots = vendors
            .iter()
            .map(|(input, v)| select(&session, *input as usize, v))
            .collect::<Result<Vec<_>>>()?;
        let link = LinkRequest {
            companions: vec![],
            revision: Some(session.revision.clone()),
            inputs: vec![0, NET80211_INPUT],
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
        let rates = RateTables {
            index: vendor_section(&session, RateTables::OBJECT, RateTables::INDEX_SECTION)?,
            arena: vendor_section(&session, RateTables::OBJECT, RateTables::ARENA_SECTION)?,
        };
        let admitted: Vec<u32> = rates
            .mapped()
            .into_iter()
            .filter(|code| !UNADMITTED_RATE_CODES.contains(code))
            .collect();
        for code in RATE_CODES {
            if !rates.covers_publication_limit(*code)? {
                return Err(invalid(format!(
                    "vendor 802.11g record of rate {code:#x} ends before its publication limit"
                )));
            }
        }
        if admitted != RATE_CODES {
            return Err(invalid(format!(
                "rcGetRate rate domain differs from the vendor index table: {:x?}",
                rates.mapped()
            )));
        }
        Ok(Self {
            rates,
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

    /// Both sides of `leaf` for every argument combination, object state and
    /// fill.
    /// Each case carries the initial bytes of the objects it compares and
    /// its probe words.
    fn cases(&mut self, leaf: &Leaf) -> Result<Vec<LeafCase>> {
        let image_symbols = image_symbols(&self.session.run.join("image/image.elf"))?;
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
                if let Some(address) = image_symbols.get(name) {
                    return Ok(*address);
                }
                Ok(u32::try_from(
                    crate::harness::symbol(
                        &self.session.inventory,
                        crate::layout::ROM_INPUT as usize,
                        name,
                    )?
                    .value,
                )?)
            };
            let states: Vec<Option<u32>> = if leaf.states.is_empty() {
                vec![None]
            } else {
                leaf.states.iter().copied().map(Some).collect()
            };
            for state in states {
                let (
                    vendor_words,
                    mut vendor_memory,
                    production_memory,
                    vendor_calls,
                    compared,
                    initial,
                    image,
                ) = match leaf.vendor_abi {
                    Some(abi) => {
                        let mut semantic = words.clone();
                        semantic.extend(state);
                        let objects = abi(
                            &semantic,
                            &Vendor {
                                resolve: &rom,
                                image: &image_symbols,
                                rates: &self.rates,
                            },
                        )?;
                        let initial = compared_bytes(&objects.vendor, &objects.compared)?;
                        (
                            objects.vendor_words,
                            regions(&objects.vendor)?,
                            regions(&objects.production)?,
                            objects.calls,
                            objects.compared,
                            initial,
                            objects.image,
                        )
                    }
                    None => (
                        words.clone(),
                        memory.clone(),
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                    ),
                };
                let initial = [vec![OUTPUT_FILL; output.unwrap_or(0) as usize], initial].concat();
                let compared: Vec<_> = compared
                    .iter()
                    .map(|(address, length)| selection(*address, *length))
                    .collect();
                vendor_memory.extend(output_memory.clone());
                let observed = [observe.clone(), compared].concat();
                let state_label = state.map_or(String::new(), |state| format!("-s{state:x}"));
                for fill in LEAF_FILLS {
                    let mut vendor = direct(
                        self.vendor_entry(leaf)?,
                        &vendor_words,
                        vendor_memory.clone(),
                        vec![radio_aperture(fill)],
                        observed.clone(),
                    );
                    vendor.calls = vendor_calls.clone();
                    for name in leaf.quiet_calls {
                        vendor.calls.push(CallDeclaration {
                            id: (*name).into(),
                            applicability: "an assertion the compared domain never fails".into(),
                            lifetime: RegionLifetime::Phase,
                            binding: CallBinding {
                                address: rom(name)?,
                                boundary: call_boundary(&image_symbols, name),
                                allow_tail: true,
                            },
                            argument_words: 1,
                            responses: vec![CallResponse {
                                return_words: [Some(0), None],
                                outputs: vec![],
                                allocation: None,
                                delay_micros: None,
                            }],
                            repetition: CallRepetition::Unbounded,
                        });
                    }
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
                        observed.clone(),
                    )?;
                    if let Some(select) = leaf.dispatch {
                        let (vendor_callee, production_callee) = select(&words);
                        let capture = CallCapture {
                            include_tail: true,
                            argument_words: 1,
                            overrides: vec![],
                        };
                        vendor.goal = ExecutionGoal::ObserveCall {
                            target: ExecutionSymbol {
                                source: self.vendor.source.clone(),
                                symbol: image_symbol_id(
                                    &self.session.run.join("image/image.elf"),
                                    &self.image_object,
                                    vendor_callee,
                                )?,
                            },
                            include_tail: true,
                        };
                        vendor.observe_calls = Some(capture.clone());
                        production.goal = ExecutionGoal::ObserveCall {
                            target: ExecutionSymbol {
                                source: self.production.source.clone(),
                                symbol: crate::harness::symbol(
                                    &self.session.inventory,
                                    PROBE_INPUT,
                                    production_callee,
                                )?
                                .id
                                .clone(),
                            },
                            include_tail: true,
                        };
                        production.observe_calls = Some(capture);
                    }
                    production.memory.extend(output_memory.clone());
                    production.memory.extend(production_memory.clone());
                    production.arguments.resize(8, Some(0));
                    let name = format!("{}{label}{state_label}-{fill:02x}", leaf.vendor);
                    // Each image object is copied in by the captured ROM
                    // `memcpy`; production has no counterpart and copies
                    // nothing.
                    let memcpy = u32::try_from(
                        crate::harness::symbol(
                            &self.session.inventory,
                            crate::layout::ROM_INPUT as usize,
                            "memcpy",
                        )?
                        .value,
                    )?;
                    let mut setup = vec![];
                    for (index, (address, bytes)) in image.iter().enumerate() {
                        let length = bytes.len() as u32;
                        let mut phase = case(
                            format!("{name}-image-{index}"),
                            direct(
                                memcpy,
                                &[*address, IMAGE_SOURCE, length],
                                vec![known(IMAGE_SOURCE, length, bytes)?],
                                vec![],
                                vec![],
                            ),
                            Some(direct(
                                memcpy,
                                &[IMAGE_SOURCE, IMAGE_SOURCE, 0],
                                vec![],
                                vec![],
                                vec![],
                            )),
                            if index == 0 {
                                SessionReset::Cold
                            } else {
                                SessionReset::Warm
                            },
                            false,
                        );
                        phase.stack_fill = Some(fill);
                        setup.push(phase);
                    }
                    let mut row = case(
                        name,
                        vendor,
                        Some(production),
                        if setup.is_empty() {
                            SessionReset::Cold
                        } else {
                            SessionReset::Warm
                        },
                        false,
                    );
                    row.stack_fill = Some(fill);
                    let relation = row.relation.as_mut().unwrap();
                    relation.returns.low = leaf.returns;
                    relation.effects = effects.clone();
                    relation.memory = (0..observed.len() as u16)
                        .map(|index| MemoryPair {
                            vendor: index,
                            replacement: index,
                        })
                        .collect();
                    rows.push((setup, row, initial.clone(), words.clone()));
                }
            }
        }
        Ok(rows)
    }
}

/// Compare every leaf; each must MATCH in every case and record effects.
pub fn exercise(ctx: &mut Mac) -> Result<()> {
    for leaf in LEAVES {
        let mut rows = vec![];
        let (mut initial, mut case_words, mut positions) = (vec![], vec![], vec![]);
        for (setup, row, bytes, words) in ctx.cases(leaf)? {
            rows.extend(setup);
            positions.push(rows.len() as u32);
            rows.push(row);
            initial.push(bytes);
            case_words.push(words);
        }
        let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
        let records = ctx
            .submit(
                leaf.probe,
                &request(&vendor, Some(&production), None, rows, LEAF_EVENTS),
                Some(ComparisonVerdict::Match),
            )?
            .records
            .clone();
        for (index, case) in positions.into_iter().enumerate() {
            for side in [false, true] {
                if !crate::i2c::all_complete(&records, case, side) {
                    return Err(invalid(format!(
                        "{} case {case} did not complete",
                        leaf.vendor
                    )));
                }
            }
            if leaf.dispatch.is_some() {
                let expected = case_words[index][0];
                for side in [false, true] {
                    let argument = crate::evidence::events(&records, case, side)
                        .iter()
                        .rev()
                        .find_map(|e| match e {
                            blobray_domain::ExecutionEvent::TransferArgument { word: 0, value } => {
                                value.value()
                            }
                            _ => None,
                        });
                    if argument != Some(expected) {
                        return Err(invalid(format!(
                            "{} case {case}: side {side} reached its callee with {argument:?}",
                            leaf.vendor
                        )));
                    }
                }
                continue;
            }
            // A leaf must act: a register effect, or a write that changes an
            // object it is compared through.
            let initial = &initial[index];
            if crate::evidence::phy_effects(&crate::evidence::events(&records, case, false))
                .is_empty()
                && (initial.is_empty()
                    || crate::evidence::output(&records, case, false) == *initial)
            {
                return Err(invalid(format!(
                    "{} case {case} has no register effect and changes no compared object",
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
