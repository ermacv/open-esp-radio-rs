//! Stateful comparison of the ordinary-MPDU retry leaves.
//!
//! One sequence starts from the vendor's own `lmacInit`, which builds the
//! queue contexts `our_instances_ptr` names, and the production retry
//! owner's reset. Warm phases then apply one completion each: the vendor
//! leaf (`lmacProcessCtsTimeout`, `lmacProcessCollision`) against the
//! production retry owner's `observe_completion`, for a short and a long
//! frame on the two sides of the vendor's RTS threshold. Every phase compares
//! the descriptor retry counters, the queue's contention exponent and the
//! frame's Retry bit, and the scenario checks that both sides choose the same
//! continuation: the vendor retries through its leaf's continuation, or ends
//! the exchange, as production retries or completes.
use crate::harness::{Arg, Result, case, direct, invalid, known, region, selection};
use crate::mac::{LEAF_FILLS, Mac};
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, ComparisonVerdict,
    ExecutionCase, ExecutionEvent, ExecutionEvidence, MemoryPair, RegionLifetime, SessionReset,
};

/// Vendor functions the sequences enter, linked as extra roots.
pub const ROOTS: &[&str] = &[
    "lmacInit",
    "lmacProcessCtsTimeout",
    "lmacProcessCollision",
    "lmacProcessAckTimeout",
];
/// Evidence claims: each vendor retry leaf with the production retry owner.
pub const CLAIMS: &[(&str, &str, &str)] = &[
    (
        "archive",
        "lmacProcessCtsTimeout",
        "open_libpp_tx_retry_trace_step",
    ),
    (
        "archive",
        "lmacProcessCollision",
        "open_libpp_tx_retry_trace_step",
    ),
    (
        "archive",
        "lmacProcessAckTimeout",
        "open_libpp_tx_retry_trace_step",
    ),
];
/// `lmacConfMib`, which `lmacInit` fills: the long and short retry limits
/// at 0x14 and 0x15 and the RTS length threshold at 0x16.
const CONF: &str = "lmacConfMib";
const CONF_LIMITS: u32 = 0x14;
const CONF_LIMITS_BYTES: u32 = 4;

/// Queue contexts `lmacInit` builds: five of 0x38 bytes.
const QUEUES: u32 = 0x3fff_2000;
const QUEUE_BYTES: u32 = 0x38;
const QUEUE_COUNT: u32 = 5;
/// Queue-context fields: the transmitting `esf_buf`, the contention
/// exponent and the exchange state, which the timeout leaves require to be
/// one (transmitting).
const QUEUE_BUFFER: u32 = 0x00;
const QUEUE_EXPONENT: u32 = 0x08;
const QUEUE_STATE: u32 = 0x12;
const QUEUE_TRANSMITTING: u8 = 1;
/// The transmitting `esf_buf`: its DMA descriptor at 0x04, its frame length
/// at 0x14 (which `lmacIsLongFrame` compares with the RTS threshold) and its
/// transmit descriptor at 0x34.
const BUFFER: u32 = 0x3fff_2400;
const BUFFER_BYTES: usize = 0x40;
const BUFFER_DMA: usize = 0x04;
const BUFFER_LENGTH: usize = 0x14;
const BUFFER_DESCRIPTOR: usize = 0x34;
/// The transmit descriptor: retry counters at 5..8 (MPDU, short, long) and
/// the rate schedule record at 0x1c.
const DESCRIPTOR: u32 = 0x3fff_2500;
const DESCRIPTOR_BYTES: usize = 0x40;
const DESCRIPTOR_COUNTERS: u32 = 5;
const DESCRIPTOR_COUNTER_BYTES: u32 = 3;
const DESCRIPTOR_RECORD: usize = 0x1c;
/// The DMA descriptor, whose word 1 is the frame, and the frame, a data
/// frame whose flags byte 1 carries the Retry bit.
const DMA: u32 = 0x3fff_2600;
const DMA_BYTES: usize = 0x10;
const DMA_FRAME: usize = 0x04;
const FRAME: u32 = 0x3fff_2700;
const FRAME_BYTES: usize = 0x20;
const FRAME_FLAGS: u32 = 1;
const FRAME_DATA: u8 = 0x08;
/// The 802.11g schedule record of the frame's initial rate.
const RECORD: u32 = 0x3fff_2800;
/// Source of the queue-context patches a setup phase copies.
const PATCH: u32 = 0x3fff_2900;
/// Legacy 6 Mbit/s, the frame's initial rate.
const INITIAL_RATE: u32 = 0x0b;
/// Production decisions the step probe returns.
const RETRY_COMPLETE: u32 = 0;
const RETRY_AGAIN: u32 = 1;
const RETRY_AGAIN_WITH_BIT: u32 = 2;
/// One vendor retry leaf: its extra arguments after the queue, the step
/// probe's disposition, and the continuation a retry reaches with its
/// third argument word when that word decides.
#[derive(Clone, Copy)]
struct Completion {
    label: &'static str,
    leaf: &'static str,
    arguments: &'static [u32],
    disposition: u32,
    retry: (&'static str, Option<u32>),
    /// The production decision of a retry: with the Retry bit or without.
    retry_decision: u32,
    /// Whether the MPDU's publication limit, and not a class limit, ends it.
    publication_limited: bool,
}

const COMPLETIONS: [Completion; 3] = [
    // `lmacProcessShortRetryFail(context, 0, 1, _)` retries through
    // `lmacEndFrameExchangeSequence(context, 1, 1)`.
    Completion {
        label: "cts",
        leaf: "lmacProcessCtsTimeout",
        arguments: &[0],
        disposition: 1,
        retry: ("lmacEndFrameExchangeSequence", Some(1)),
        retry_decision: RETRY_AGAIN,
        publication_limited: false,
    },
    // Both collision branches retry through `lmacRetryTxFrame`.
    Completion {
        label: "collision",
        leaf: "lmacProcessCollision",
        arguments: &[],
        disposition: 2,
        retry: ("lmacRetryTxFrame", None),
        retry_decision: RETRY_AGAIN,
        publication_limited: false,
    },
    // `lmacProcessShortRetryFail(context, 0, 0, 0)` sets the Retry bit and
    // retries through `lmacEndFrameExchangeSequence(context, 1, 1)`; the
    // record's publication limit (`rcReachRetryLimit`) ends the MPDU.
    Completion {
        label: "ack",
        leaf: "lmacProcessAckTimeout",
        arguments: &[0],
        disposition: 0,
        retry: ("lmacEndFrameExchangeSequence", Some(1)),
        retry_decision: RETRY_AGAIN_WITH_BIT,
        publication_limited: true,
    },
];
/// The vendor's own limits: long and short retry limits and the RTS length
/// threshold.
#[derive(Clone, Copy)]
struct Limits {
    long: u8,
    short: u8,
    threshold: u16,
}
/// Guest events one retry phase may record.
const RETRY_EVENTS: u32 = 1 << 12;
/// The ordinary queues: VO, VI, BE and BK.
const QUEUES_COMPARED: [u32; 4] = [0, 1, 2, 3];
/// Vendor functions answered without effect during `lmacInit`: rate-control
/// setup and the floating-point HE A-MPDU limit tables `lmacInitAc` ends
/// with, none of which the retry leaves read.
const INIT_QUIET: &[&str] = &[
    "RC_SetBasicRate",
    "rcAttach",
    "rx11AXRate2AMPDULimit_update",
];
/// Vendor functions answered with zero during a retry leaf: no MSDU ages,
/// no MU EDCA, no test statistics and no trigger-based success.
const RETRY_QUIET: &[&str] = &[
    "lmacMSDUAged",
    "is_use_muedca",
    "esp_test_tx_count_retry",
    "lmacProcessTBSuccess",
    "wifi_assert",
];
/// The continuations a retry leaf ends in.
const CONTINUATIONS: &[&str] = &[
    "lmacEndFrameExchangeSequence",
    "lmacDiscardFrameExchangeSequence",
    "lmacRetryTxFrame",
];

fn model(name: &str, address: u32, boundary: CallBoundary, words: u16) -> CallDeclaration {
    model_with(name, address, boundary, words, vec![])
}

/// `lmacRetryTxFrame` republishes the frame, which returns the queue to its
/// transmitting state. A phase that must retry declares it for exactly one
/// call, so a vendor that does not retry leaves it incomplete.
fn retry_model(address: u32, boundary: CallBoundary) -> CallDeclaration {
    let mut model = model_with(
        "lmacRetryTxFrame",
        address,
        boundary,
        3,
        vec![blobray_domain::CallOutput {
            pointer_argument: 0,
            byte_offset: QUEUE_STATE,
            width: 1,
            value: u32::from(QUEUE_TRANSMITTING),
            scope: blobray_domain::CallOutputScope::NormalMemory,
        }],
    );
    model.repetition = CallRepetition::Finite;
    model
}

fn model_with(
    name: &str,
    address: u32,
    boundary: CallBoundary,
    words: u16,
    outputs: Vec<blobray_domain::CallOutput>,
) -> CallDeclaration {
    CallDeclaration {
        id: name.into(),
        applicability: "a retry-leaf continuation or setup callee answered without effect".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary,
            allow_tail: true,
        },
        argument_words: words,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs,
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// One retry sequence of `queue`: the vendor `lmacInit` and production reset,
/// the queue-context patch, and one compared `completion` phase per attempt
/// until the frame's retry limit ends it.
#[allow(clippy::too_many_arguments)]
fn sequence(
    ctx: &Mac,
    completion: Completion,
    queue: u32,
    fill: u8,
    limit: u8,
    limits: Limits,
    long: bool,
) -> Result<Vec<(ExecutionCase, bool)>> {
    let image = ctx.image_symbols()?;
    let symbol = |name: &str| ctx.symbol_address(&image, name);
    let boundary = |name: &str| crate::mac::call_boundary(&image, name);
    let context = QUEUES + queue * QUEUE_BYTES;
    let record = ctx
        .rates
        .record(crate::mac::RateArena::Legacy, INITIAL_RATE)?;
    let mut buffer = vec![0u8; BUFFER_BYTES];
    buffer[BUFFER_DMA..BUFFER_DMA + 4].copy_from_slice(&DMA.to_le_bytes());
    buffer[BUFFER_DESCRIPTOR..BUFFER_DESCRIPTOR + 4].copy_from_slice(&DESCRIPTOR.to_le_bytes());
    // A long frame is one byte over the threshold; a short one reaches it.
    let length = limits.threshold + u16::from(long);
    buffer[BUFFER_LENGTH..BUFFER_LENGTH + 2].copy_from_slice(&length.to_le_bytes());
    let mut descriptor = vec![0u8; DESCRIPTOR_BYTES];
    descriptor[DESCRIPTOR_RECORD..DESCRIPTOR_RECORD + 4].copy_from_slice(&RECORD.to_le_bytes());
    let mut dma = vec![0u8; DMA_BYTES];
    dma[DMA_FRAME..DMA_FRAME + 4].copy_from_slice(&FRAME.to_le_bytes());
    let mut frame = vec![0u8; FRAME_BYTES];
    frame[0] = FRAME_DATA;
    let session = |address: u32, bytes: &[u8]| {
        region(
            address,
            bytes.len() as u32,
            bytes,
            None,
            RegionLifetime::Session,
        )
    };
    let queues = vec![0u8; (QUEUE_BYTES * QUEUE_COUNT) as usize];
    // Cold: the vendor builds its queue contexts; production resets.
    let init = init_invocation(
        ctx,
        &image,
        vec![
            session(QUEUES, &queues)?,
            session(BUFFER, &buffer)?,
            session(DESCRIPTOR, &descriptor)?,
            session(DMA, &dma)?,
            session(FRAME, &frame)?,
            session(RECORD, &record)?,
        ],
        vec![],
    )?;
    let reset = ctx.session.probes.invoke(
        "open_libpp_tx_retry_trace_reset",
        vec![
            ("queue", Arg::Word(Some(i64::from(queue)))),
            ("mpdu_retry_limit", Arg::Word(Some(i64::from(limit)))),
            ("long_frame", Arg::Word(Some(i64::from(long)))),
        ],
        vec![],
        vec![],
    )?;
    let mut reset = reset;
    reset.memory.extend([
        session(QUEUES, &queues)?,
        session(DESCRIPTOR, &descriptor)?,
        session(FRAME, &frame)?,
    ]);
    reset.arguments.resize(8, Some(0));
    let mut rows = vec![(
        case(
            format!("retry-{}-init-q{queue}-{long}-{fill:02x}", completion.label),
            init,
            Some(reset),
            SessionReset::Cold,
            false,
        ),
        false,
    )];
    // Warm: the vendor's transmit path installs the frame and marks the
    // queue transmitting; production has no counterpart.
    let memcpy = u32::try_from(
        crate::harness::symbol(
            &ctx.session.inventory,
            crate::layout::ROM_INPUT as usize,
            "memcpy",
        )?
        .value,
    )?;
    let patches: [(u32, Vec<u8>); 2] = [
        (context + QUEUE_BUFFER, BUFFER.to_le_bytes().to_vec()),
        (context + QUEUE_STATE, vec![QUEUE_TRANSMITTING]),
    ];
    for (index, (address, bytes)) in patches.iter().enumerate() {
        let length = bytes.len() as u32;
        rows.push((
            case(
                format!(
                    "retry-{}-patch-q{queue}-{long}-{index}-{fill:02x}",
                    completion.label
                ),
                direct(
                    memcpy,
                    &[*address, PATCH, length],
                    vec![known(PATCH, length, bytes)?],
                    vec![],
                    vec![],
                ),
                Some(direct(memcpy, &[PATCH, PATCH, 0], vec![], vec![], vec![])),
                SessionReset::Warm,
                false,
            ),
            false,
        ));
    }
    let named = |name: &str, address: u32, length: u32| blobray_domain::MemorySelection {
        name: name.into(),
        address,
        length,
    };
    let observed = vec![
        named(
            "descriptor-retry-counters",
            DESCRIPTOR + DESCRIPTOR_COUNTERS,
            DESCRIPTOR_COUNTER_BYTES,
        ),
        named("queue-contention-exponent", context + QUEUE_EXPONENT, 1),
        named("frame-flags", FRAME + FRAME_FLAGS, 1),
    ];
    let leaf = symbol(completion.leaf)?;
    // A CTS timeout always counts as short; a collision counts in the frame's
    // class.
    let attempts = if completion.publication_limited {
        limit
    } else if long && completion.disposition != COMPLETIONS[0].disposition {
        limits.long
    } else {
        limits.short
    };
    let arguments: Vec<u32> = [queue]
        .iter()
        .chain(completion.arguments)
        .copied()
        .collect();
    for step in 0..u32::from(attempts) {
        let mut vendor = direct(leaf, &arguments, vec![], vec![], observed.clone());
        for name in RETRY_QUIET {
            vendor
                .calls
                .push(model(name, symbol(name)?, boundary(name), 1));
        }
        let retries = step + 1 < u32::from(attempts) && completion.retry.0 == "lmacRetryTxFrame";
        for name in CONTINUATIONS {
            vendor
                .calls
                .push(if *name == "lmacRetryTxFrame" && retries {
                    retry_model(symbol(name)?, boundary(name))
                } else {
                    model(name, symbol(name)?, boundary(name), 3)
                });
        }
        let mut production = ctx.session.probes.invoke(
            "open_libpp_tx_retry_trace_step",
            vec![
                (
                    "disposition",
                    Arg::Word(Some(i64::from(completion.disposition))),
                ),
                ("context", Arg::Word(Some(i64::from(context)))),
                ("descriptor", Arg::Word(Some(i64::from(DESCRIPTOR)))),
                ("buffer", Arg::Word(Some(i64::from(FRAME)))),
            ],
            vec![],
            observed.clone(),
        )?;
        production.arguments.resize(8, Some(0));
        let mut row = case(
            format!(
                "retry-{}-q{queue}-{long}-{step}-{fill:02x}",
                completion.label
            ),
            vendor,
            Some(production),
            SessionReset::Warm,
            false,
        );
        let relation = row.relation.as_mut().unwrap();
        relation.memory = (0..observed.len() as u16)
            .map(|index| MemoryPair {
                vendor: index,
                replacement: index,
            })
            .collect();
        rows.push((row, true));
    }
    for (row, _) in &mut rows {
        row.stack_fill = Some(fill);
    }
    Ok(rows)
}

/// The vendor `lmacInit` with `memory`, its queue-context pointer cell and
/// its rate-control setup answered without effect.
fn init_invocation(
    ctx: &Mac,
    image: &std::collections::BTreeMap<String, u32>,
    mut memory: Vec<blobray_domain::ExecutionRegion>,
    observe: Vec<blobray_domain::MemorySelection>,
) -> Result<blobray_domain::Invocation> {
    let symbol = |name: &str| ctx.symbol_address(image, name);
    memory.push(region(
        symbol("our_instances_ptr")?,
        4,
        &QUEUES.to_le_bytes(),
        None,
        RegionLifetime::Session,
    )?);
    let mut init = direct(symbol("lmacInit")?, &[], memory, vec![], observe);
    for name in INIT_QUIET {
        init.calls.push(model(
            name,
            symbol(name)?,
            crate::mac::call_boundary(image, name),
            2,
        ));
    }
    Ok(init)
}

/// The retry limits and RTS threshold the vendor's `lmacInit` installs.
fn vendor_limits(ctx: &mut Mac) -> Result<Limits> {
    let image = ctx.image_symbols()?;
    let queues = vec![0u8; (QUEUE_BYTES * QUEUE_COUNT) as usize];
    let address = ctx.symbol_address(&image, CONF)? + CONF_LIMITS;
    let init = init_invocation(
        ctx,
        &image,
        vec![region(
            QUEUES,
            queues.len() as u32,
            &queues,
            None,
            RegionLifetime::Session,
        )?],
        vec![selection(address, CONF_LIMITS_BYTES)],
    )?;
    let mut row = case("lmac-init", init, None, SessionReset::Cold, false);
    row.relation = None;
    row.stack_fill = Some(LEAF_FILLS[0]);
    let vendor = ctx.vendor.clone();
    let records = ctx
        .submit(
            "lmac-init",
            &crate::session::request(&vendor, None, None, vec![row], RETRY_EVENTS),
            None,
        )?
        .records
        .clone();
    // The vendor must return from lmacInit, not stop incomplete.
    crate::i2c::returned_low(&records, 0, false);
    let [long, short, low, high] = crate::evidence::output(&records, 0, false)[..] else {
        return Err(invalid("lmacInit left no retry limits"));
    };
    Ok(Limits {
        long,
        short,
        threshold: u16::from_le_bytes([low, high]),
    })
}

/// The vendor continuation of one phase: the modeled callee and its third
/// argument word.
fn continuation(
    records: &[ExecutionEvidence],
    case: u32,
    targets: &[(u32, &'static str)],
) -> Option<(&'static str, Option<u32>)> {
    let events = crate::evidence::events(records, case, false);
    let mut found = None;
    let mut current: Option<&'static str> = None;
    for event in events {
        match event {
            ExecutionEvent::ModeledCall { target, .. } => {
                current = targets.iter().find(|(a, _)| *a == target).map(|(_, n)| *n);
                if let Some(name) = current {
                    found = Some((name, None));
                }
            }
            ExecutionEvent::CallArgument { word: 2, value } if current.is_some() => {
                found = Some((current.unwrap(), value));
            }
            _ => {}
        }
    }
    found
}

/// Compare every retry sequence; each phase must MATCH and choose the
/// continuation production chooses.
pub fn exercise(ctx: &mut Mac) -> Result<()> {
    let limit = ctx.rates.publication_limit(INITIAL_RATE)?;
    let limits = vendor_limits(ctx)?;
    let image = ctx.image_symbols()?;
    let targets: Vec<(u32, &'static str)> = CONTINUATIONS
        .iter()
        .map(|name| Ok((ctx.symbol_address(&image, name)?, *name)))
        .collect::<Result<_>>()?;
    for completion in COMPLETIONS {
        for queue in QUEUES_COMPARED {
            let mut rows = vec![];
            // Compared phases of each sequence, in order.
            let mut sequences = vec![];
            for long in [false, true] {
                for fill in LEAF_FILLS {
                    let mut compared = vec![];
                    for (row, is_compared) in
                        sequence(ctx, completion, queue, fill, limit, limits, long)?
                    {
                        if is_compared {
                            compared.push(rows.len() as u32);
                        }
                        rows.push(row);
                    }
                    sequences.push(compared);
                }
            }
            let label = format!("retry-{}-q{queue}", completion.label);
            let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
            let records = ctx
                .submit(
                    &label,
                    &crate::session::request(&vendor, Some(&production), None, rows, RETRY_EVENTS),
                    Some(ComparisonVerdict::Match),
                )?
                .records
                .clone();
            for compared in sequences {
                for (phase, case) in compared.iter().enumerate() {
                    let last = phase + 1 == compared.len();
                    let decision = crate::i2c::returned_low(&records, *case, true);
                    // Each sequence retries until its last phase completes it.
                    let expected = if last {
                        RETRY_COMPLETE
                    } else {
                        completion.retry_decision
                    };
                    if decision != Some(expected) {
                        return Err(invalid(format!(
                            "{label} case {case}: production decided {decision:?} at phase {phase}"
                        )));
                    }
                    let reached = continuation(&records, *case, &targets);
                    let (callee, flag) = completion.retry;
                    let retried = matches!(
                        reached,
                        Some((name, word)) if name == callee && (flag.is_none() || word == flag)
                    );
                    if reached.is_none() || retried == last {
                        return Err(invalid(format!(
                            "{label} case {case}: production decided {decision:?}, the vendor reached {reached:?}"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
