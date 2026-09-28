//! Comparison of the A-MPDU BlockAck completion with `ppResortTxAMPDU`.
//!
//! The vendor resorts one queue's completed aggregate after
//! `ppTxqUpdateBitmap` stored the BlockAck's starting sequence and bitmap in
//! the queue context: every MPDU the bitmap acknowledges is recycled, and
//! every other one is kept for the next aggregate with the IEEE 802.11 Retry
//! bit set in its frame. Production observes the same completion through its
//! retained A-MPDU owner and retry state and, when it retains the aggregate,
//! sets the Retry bit of every missing MPDU in that MPDU's own DMA backing.
//! Both sides hold the same encoded MPDUs; each case compares every MPDU's
//! header after the completion.
use crate::harness::{Arg, Result, case, direct, invalid, known, selection};
use crate::mac::{LEAF_FILLS, Mac};
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, ComparisonVerdict,
    EventChannels, ExecutionCase, MemoryPair, MemorySelection, RegionLifetime, ReturnWords,
    SessionReset,
};

/// The vendor function, linked as an extra root.
pub const ROOTS: &[&str] = &[VENDOR];
/// Evidence claim: the vendor resort with the production completion and
/// retry retention.
pub const CLAIMS: &[(&str, &str, &str)] = &[("archive", VENDOR, RESORT_PROBE)];
const VENDOR: &str = "ppResortTxAMPDU";
const LAYOUT_PROBE: &str = "open_libpp_ampdu_trace_layout";
const RESORT_PROBE: &str = "open_libpp_ampdu_trace_resort";

/// The ROM pointer to the transmit context whose queue records the resort
/// reads, one of `QUEUE_STRIDE` bytes per queue.
const TX_RX: &str = "pTxRx";
const QUEUE_STRIDE: u32 = 0x34;
/// Queue-record fields: the pending count byte, the queue-valid byte, the
/// BlockAck starting sequence, the TID, the resorted aggregate's first
/// `esf_buf`, and the bitmap's high and low words.
const QUEUE_COUNT: u32 = 0x28;
const QUEUE_VALID: u32 = 0x29;
const QUEUE_STARTING_SEQUENCE: u32 = 0x2a;
const QUEUE_TID: u32 = 0x2c;
const QUEUE_AGGREGATE: u32 = 0x34;
const QUEUE_BITMAP_HIGH: u32 = 0x38;
const QUEUE_BITMAP_LOW: u32 = 0x3c;
/// Bytes of the transmit context the queue records occupy.
const TX_RX_BYTES: u32 = 0x200;
/// The queue the aggregate completed on: best effort, TID zero.
const QUEUE: u32 = 2;
const TID: u8 = 0;

/// `esf_buf` fields: the first and the last DMA descriptor, the frame length,
/// the buffer flags, the station context, the next buffer and the transmit
/// descriptor. Flag 0x2000 marks a buffer that starts with the eight-byte
/// A-MPDU metadata before the frame.
const ESF_BYTES: u32 = 0x40;
const ESF_DMA: usize = 0x04;
const ESF_LAST_DMA: usize = 0x08;
const ESF_LENGTH: usize = 0x14;
const ESF_FLAGS: usize = 0x24;
const ESF_AMPDU_METADATA: u16 = 0x2000;
const ESF_STATION: usize = 0x2c;
const ESF_NEXT: usize = 0x30;
const ESF_DESCRIPTOR: usize = 0x34;
/// DMA descriptor: word 0 the buffer size and length with the end-of-frame
/// flag, word 1 the buffer, word 2 the next descriptor of the aggregate.
const DMA_BYTES: u32 = 0x10;
const DMA_FRAME: usize = 0x04;
const DMA_NEXT: usize = 0x08;
const DMA_LENGTH_SHIFT: u32 = 12;
const DMA_END_OF_FRAME: u32 = 1 << 30;
/// Transmit descriptor word 0 of an aggregate member: the A-MPDU marker,
/// and the first member's additional marker.
const DESCRIPTOR_BYTES: u32 = 0x40;
const DESCRIPTOR_AMPDU: u32 = 0x0040_0000;
const DESCRIPTOR_FIRST: u32 = 0x0008_0000;
/// The access context `GetAccess` returns and the station context, whose
/// byte at `STATION_BAR_PENDING` holds one BlockAckReq-pending bit per TID.
const ACCESS_BYTES: u32 = 0x80;
const STATION_BYTES: u32 = 0x40;
const STATION_BAR_PENDING: usize = 0x28;
/// Vendor arenas.
const ARENA_TX_RX: u32 = 0x3fff_a000;
const ARENA_ACCESS: u32 = 0x3fff_a200;
const ARENA_STATION: u32 = 0x3fff_a300;
const ARENA_ESF: u32 = 0x3fff_a400;
const ARENA_DMA: u32 = 0x3fff_a800;
const ARENA_DESCRIPTOR: u32 = 0x3fff_a900;
const ARENA_FRAMES: u32 = 0x3fff_b000;
/// Production input frames and output words.
const PRODUCTION_FRAMES: u32 = 0x3fff_c000;
const PRODUCTION_OUTPUT: u32 = 0x3fff_c400;
const OUTPUT_WORDS: u32 = 4;
const LAYOUT_OUTPUT: u32 = 0x3fff_c800;

/// Bytes of one encoded MPDU, as the probe commits it, and of the header
/// each case compares: a QoS data header through QoS Control.
const MPDU_BYTES: usize = 32;
/// The vendor A-MPDU metadata before each frame: word 0 carries the PSDU
/// length in bits 0..14 and the MPDU's sequence number low byte in bits
/// 16..24, which `ppResortTxAMPDU` measures against the BlockAck starting
/// sequence; byte 4 the empty delimiters. The PSDU adds the frame check
/// sequence.
const METADATA_BYTES: usize = 8;
const METADATA_SEQUENCE_SHIFT: u32 = 16;
const FCS_BYTES: usize = 4;
const BUFFER_BYTES: usize = METADATA_BYTES + MPDU_BYTES;
const HEADER_BYTES: u32 = 26;
const SLOTS: usize = 8;
/// A QoS data frame to the AP, and the header offsets of Sequence Control.
const FRAME_CONTROL: [u8; 2] = [0x88, 0x01];
const SEQUENCE_CONTROL: usize = 22;
const SEQUENCE_SHIFT: u16 = 4;
/// Frame Control's flags byte and its Retry bit.
const RETRY_BYTE: u32 = 1;
const RETRY: u8 = 0x08;
/// Guest events one resort may record.
const RESORT_EVENTS: u32 = 1 << 12;

/// Vendor callees answered with a constant: rate control, recycling,
/// the next transmission and the BlockAck request are outside the compared
/// frame state.
const ANSWERED: &[(&str, u32)] = &[
    ("rcUpdateTxDoneAmpdu2", 0),
    ("trc_isTxAmpduOperational", 1),
    ("trc_tid_isTxAmpduOperational", 1),
    ("lmacMSDUAged", 0),
    ("lmacRecycleMPDU", 0),
    ("lmacDiscardAgedMSDU", 0),
    ("esp_test_tx_count_retry", 0),
    ("ppResumeTxAMPDU", 0),
    ("ppProcessTxQ", 0),
    ("ppFillAMPDUBar", 0),
    ("ppReSendBar", 0),
    ("ppHEAMPDU2Normal", 0),
    ("wifi_log", 0),
];

/// Reviewed differences: production admits the MPDU left of the starting
/// sequence as acknowledged, and hands a single missing HT MPDU to its
/// ordinary retry owner.
const PREDECESSOR: &str = "Production admits the MPDU immediately left of SSN (within the \
    64-entry window) as acknowledged, because peers use both standard-compliant SSN \
    conventions and one advances SSN to the first unacknowledged MPDU. The vendor measures \
    low-byte distance and retries that MPDU, retransmitting an already delivered frame. \
    Production suppresses that duplicate; a sequence beyond the window stays unacknowledged in \
    both (reviewed with the Wi-Fi owner)";
const HT_SINGLE: &str = "production ends the HT aggregate when one MPDU is missing and hands \
    it to its ordinary retry owner, which sets the Retry bit when it republishes the MPDU; the \
    vendor keeps it in the aggregate and sets the bit here (reviewed with the Wi-Fi owner)";
/// Known gap: production sends no BlockAckReq.
const BAR_GAP: &str = "the vendor sends a BlockAckReq (ppFillAMPDUBar, ppReSendBar) after a \
    resort whose station has a pending request for the TID; production sends none, so the \
    recipient's window advances only by its own timeout (known gap, reviewed with the Wi-Fi \
    owner)";

/// One completion: the aggregate's MPDUs, the BlockAck the peer sent, the
/// production retry policy for a single missing MPDU, whether the station
/// has a BlockAckReq pending for the TID, and the expected verdict with its
/// reviewed reason when it is a difference.
#[derive(Clone, Copy)]
struct Completion {
    label: &'static str,
    count: usize,
    first_sequence: u16,
    starting_sequence: u16,
    bitmap: u64,
    retain_single: bool,
    bar_pending: bool,
    /// A reviewed difference: its reason and the MPDU whose Retry bit
    /// differs.
    difference: Option<(&'static str, u16)>,
}

const fn completion(
    label: &'static str,
    count: usize,
    first_sequence: u16,
    starting_sequence: u16,
    bitmap: u64,
) -> Completion {
    Completion {
        label,
        count,
        first_sequence,
        starting_sequence,
        bitmap,
        retain_single: true,
        bar_pending: false,
        difference: None,
    }
}

const COMPLETIONS: &[Completion] = &[
    completion("all-acknowledged", 4, 100, 100, 0b1111),
    completion("none-acknowledged", 4, 100, 100, 0),
    completion("holes", 4, 100, 100, 0b0101),
    completion("tail-missing", 8, 200, 200, 0b0011_1111),
    // A starting sequence beyond the window acknowledges nothing.
    completion("beyond-window", 4, 100, 170, u64::MAX),
    // Sequence numbers wrap from 4095 to 0 inside the aggregate.
    completion("wrap", 4, 4094, 4094, 0b0110),
    // HE keeps a single missing MPDU in the aggregate.
    completion("single-missing-he", 4, 100, 100, 0b1011),
    Completion {
        retain_single: false,
        difference: Some((HT_SINGLE, 2)),
        ..completion("single-missing-ht", 4, 100, 100, 0b1011)
    },
    // The peer advanced SSN past the first MPDU, already delivered.
    Completion {
        difference: Some((PREDECESSOR, 0)),
        ..completion("predecessor", 4, 100, 101, 0b0101)
    },
    Completion {
        bar_pending: true,
        ..completion("bar-pending", 4, 100, 100, 0b0101)
    },
];

fn answered(name: &str, address: u32, boundary: CallBoundary, value: u32) -> CallDeclaration {
    CallDeclaration {
        id: name.into(),
        applicability: "a resort callee outside the compared frame state".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary,
            allow_tail: true,
        },
        argument_words: 4,
        responses: vec![CallResponse {
            return_words: [Some(value), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// The encoded MPDU at `index` of an aggregate starting at `first`.
fn frame(first: u16, index: usize) -> Vec<u8> {
    let mut frame = vec![0u8; MPDU_BYTES];
    frame[..2].copy_from_slice(&FRAME_CONTROL);
    for (offset, byte) in frame[4..22].iter_mut().enumerate() {
        *byte = 0x10 + offset as u8;
    }
    let sequence = first.wrapping_add(index as u16) & 0x0fff;
    frame[SEQUENCE_CONTROL..SEQUENCE_CONTROL + 2]
        .copy_from_slice(&(sequence << SEQUENCE_SHIFT).to_le_bytes());
    for (offset, byte) in frame[26..].iter_mut().enumerate() {
        *byte = index as u8 * 0x10 + offset as u8;
    }
    frame
}

fn word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Where the production probe keeps each MPDU, from its layout entry.
fn layout(ctx: &mut Mac) -> Result<Vec<u32>> {
    let mut production = ctx.session.probes.invoke(
        LAYOUT_PROBE,
        vec![("output", Arg::Word(Some(i64::from(LAYOUT_OUTPUT))))],
        vec![],
        vec![selection(LAYOUT_OUTPUT, SLOTS as u32 * 4)],
    )?;
    production
        .memory
        .push(known(LAYOUT_OUTPUT, SLOTS as u32 * 4, &[0; SLOTS * 4])?);
    production.arguments.resize(8, Some(0));
    let memcpy = u32::try_from(
        crate::harness::symbol(
            &ctx.session.inventory,
            crate::layout::ROM_INPUT as usize,
            "memcpy",
        )?
        .value,
    )?;
    let vendor = direct(
        memcpy,
        &[LAYOUT_OUTPUT, LAYOUT_OUTPUT, 0],
        vec![],
        vec![],
        vec![],
    );
    let mut row = crate::harness::setup(
        "ampdu-resort-layout",
        vendor,
        production,
        SessionReset::Cold,
    );
    row.stack_fill = Some(LEAF_FILLS[0]);
    let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
    let records = ctx
        .submit(
            "ampdu-resort-layout",
            &crate::session::request(&vendor, Some(&production), None, vec![row], RESORT_EVENTS),
            Some(ComparisonVerdict::Match),
        )?
        .records
        .clone();
    if crate::i2c::returned_low(&records, 0, true) != Some(0) {
        return Err(invalid("the A-MPDU layout entry failed"));
    }
    Ok(crate::evidence::output(&records, 0, true)
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .collect())
}

/// The compared case of `completion`.
fn case_row(ctx: &mut Mac, frames: &[u32], completion: Completion) -> Result<ExecutionCase> {
    let image = ctx.image_symbols()?;
    let symbol = |name: &str| ctx.symbol_address(&image, name);
    let count = completion.count;
    let encoded: Vec<Vec<u8>> = (0..count)
        .map(|index| frame(completion.first_sequence, index))
        .collect();

    // The vendor transmit context, access and station contexts and one
    // `esf_buf`, DMA descriptor, transmit descriptor and queue word per MPDU.
    let mut tx_rx = vec![0u8; TX_RX_BYTES as usize];
    let record = (QUEUE * QUEUE_STRIDE) as usize;
    tx_rx[record + QUEUE_COUNT as usize] = count as u8;
    tx_rx[record + QUEUE_VALID as usize] = 1;
    tx_rx[record + QUEUE_STARTING_SEQUENCE as usize..record + QUEUE_STARTING_SEQUENCE as usize + 2]
        .copy_from_slice(&completion.starting_sequence.to_le_bytes());
    tx_rx[record + QUEUE_TID as usize] = TID;
    word(&mut tx_rx, record + QUEUE_AGGREGATE as usize, ARENA_ESF);
    word(
        &mut tx_rx,
        record + QUEUE_BITMAP_HIGH as usize,
        (completion.bitmap >> 32) as u32,
    );
    word(
        &mut tx_rx,
        record + QUEUE_BITMAP_LOW as usize,
        completion.bitmap as u32,
    );
    let mut station = vec![0u8; STATION_BYTES as usize];
    if completion.bar_pending {
        station[STATION_BAR_PENDING] = 1 << TID;
    }
    let mut memory = vec![
        known(symbol(TX_RX)?, 4, &ARENA_TX_RX.to_le_bytes())?,
        known(ARENA_TX_RX, TX_RX_BYTES, &tx_rx)?,
        known(ARENA_ACCESS, ACCESS_BYTES, &vec![0; ACCESS_BYTES as usize])?,
        known(ARENA_STATION, STATION_BYTES, &station)?,
    ];
    let mut esf = vec![0u8; count * ESF_BYTES as usize];
    let mut dma = vec![0u8; count * DMA_BYTES as usize];
    let mut descriptors = vec![0u8; count * DESCRIPTOR_BYTES as usize];
    let mut frames_vendor = vec![0u8; count * BUFFER_BYTES];
    for index in 0..count {
        let buffer = &mut esf[index * ESF_BYTES as usize..(index + 1) * ESF_BYTES as usize];
        word(buffer, ESF_DMA, ARENA_DMA + index as u32 * DMA_BYTES);
        word(buffer, ESF_LAST_DMA, ARENA_DMA + index as u32 * DMA_BYTES);
        buffer[ESF_LENGTH..ESF_LENGTH + 2].copy_from_slice(&(MPDU_BYTES as u16).to_le_bytes());
        buffer[ESF_FLAGS..ESF_FLAGS + 2].copy_from_slice(&ESF_AMPDU_METADATA.to_le_bytes());
        word(buffer, ESF_STATION, ARENA_STATION);
        let next = if index + 1 < count {
            ARENA_ESF + (index as u32 + 1) * ESF_BYTES
        } else {
            0
        };
        word(buffer, ESF_NEXT, next);
        word(
            buffer,
            ESF_DESCRIPTOR,
            ARENA_DESCRIPTOR + index as u32 * DESCRIPTOR_BYTES,
        );
        let descriptor = &mut dma[index * DMA_BYTES as usize..];
        let size = BUFFER_BYTES as u32;
        let end = if index + 1 == count {
            DMA_END_OF_FRAME
        } else {
            0
        };
        word(descriptor, 0, size | size << DMA_LENGTH_SHIFT | end);
        word(
            descriptor,
            DMA_FRAME,
            ARENA_FRAMES + (index * BUFFER_BYTES) as u32,
        );
        let next_dma = if index + 1 < count {
            ARENA_DMA + (index as u32 + 1) * DMA_BYTES
        } else {
            0
        };
        word(descriptor, DMA_NEXT, next_dma);
        let flags = DESCRIPTOR_AMPDU | if index == 0 { DESCRIPTOR_FIRST } else { 0 };
        word(
            &mut descriptors[index * DESCRIPTOR_BYTES as usize..],
            0,
            flags,
        );
        let buffer = &mut frames_vendor[index * BUFFER_BYTES..(index + 1) * BUFFER_BYTES];
        let sequence = u32::from(completion.first_sequence.wrapping_add(index as u16) & 0xff);
        let metadata = (MPDU_BYTES + FCS_BYTES) as u32 | sequence << METADATA_SEQUENCE_SHIFT;
        word(buffer, 0, metadata);
        buffer[METADATA_BYTES..].copy_from_slice(&encoded[index]);
    }
    memory.extend([
        known(ARENA_ESF, esf.len() as u32, &esf)?,
        known(ARENA_DMA, dma.len() as u32, &dma)?,
        known(ARENA_DESCRIPTOR, descriptors.len() as u32, &descriptors)?,
        known(ARENA_FRAMES, frames_vendor.len() as u32, &frames_vendor)?,
    ]);
    let header = |name: String, address: u32| MemorySelection {
        name,
        address,
        length: HEADER_BYTES,
    };
    let vendor_observed: Vec<MemorySelection> = (0..count)
        .map(|index| {
            header(
                format!("mpdu-{index}-header"),
                ARENA_FRAMES + (index * BUFFER_BYTES + METADATA_BYTES) as u32,
            )
        })
        .collect();
    let mut vendor = direct(symbol(VENDOR)?, &[QUEUE], memory, vec![], vendor_observed);
    let access = symbol("GetAccess")?;
    vendor.calls.push(answered(
        "GetAccess",
        access,
        crate::mac::call_boundary(&image, "GetAccess"),
        ARENA_ACCESS,
    ));
    for (name, value) in ANSWERED {
        vendor.calls.push(answered(
            name,
            symbol(name)?,
            crate::mac::call_boundary(&image, name),
            *value,
        ));
    }

    let production_observed: Vec<MemorySelection> = (0..count)
        .map(|index| header(format!("mpdu-{index}-header"), frames[index]))
        .collect();
    let mut production = ctx.session.probes.invoke(
        RESORT_PROBE,
        vec![
            ("frames", Arg::Word(Some(i64::from(PRODUCTION_FRAMES)))),
            ("count", Arg::Word(Some(count as i64))),
            (
                "first_sequence",
                Arg::Word(Some(i64::from(completion.first_sequence))),
            ),
            (
                "starting_sequence",
                Arg::Word(Some(i64::from(completion.starting_sequence))),
            ),
            (
                "bitmap_low",
                Arg::Word(Some(i64::from(completion.bitmap as u32))),
            ),
            (
                "bitmap_high",
                Arg::Word(Some(i64::from((completion.bitmap >> 32) as u32))),
            ),
            ("received", Arg::Word(Some(1))),
            ("attempt_limit", Arg::Word(Some(4))),
            (
                "retain_single",
                Arg::Word(Some(i64::from(completion.retain_single))),
            ),
            ("output", Arg::Word(Some(i64::from(PRODUCTION_OUTPUT)))),
        ],
        vec![],
        production_observed,
    )?;
    let joined: Vec<u8> = encoded.concat();
    production.memory.extend([
        known(PRODUCTION_FRAMES, joined.len() as u32, &joined)?,
        known(
            PRODUCTION_OUTPUT,
            OUTPUT_WORDS * 4,
            &vec![0; (OUTPUT_WORDS * 4) as usize],
        )?,
    ]);
    let mut row = case(
        format!("ampdu-resort-{}", completion.label),
        vendor,
        Some(production),
        SessionReset::Cold,
        false,
    );
    let relation = row.relation.as_mut().expect("a compared case");
    relation.events = EventChannels {
        timeline: crate::harness::TIMELINE,
        mmio_read: false,
        mmio_write: false,
        fence: false,
        delay: false,
    };
    relation.returns = ReturnWords {
        low: false,
        high: false,
    };
    relation.memory = (0..count as u16)
        .map(|index| MemoryPair {
            vendor: index,
            replacement: index,
        })
        .collect();
    row.stack_fill = Some(LEAF_FILLS[0]);
    Ok(row)
}

/// Compare every completion: each matches, or differs for its reviewed
/// reason; production succeeds in every case, and the vendor sends its
/// BlockAckReq exactly when the station has one pending.
pub fn exercise(ctx: &mut Mac) -> Result<()> {
    let frames = layout(ctx)?;
    let image = ctx.image_symbols()?;
    let request_bar = ctx.symbol_address(&image, "ppReSendBar")?;
    for completion in COMPLETIONS {
        let row = case_row(ctx, &frames, *completion)?;
        let label = format!("ampdu-resort-{}", completion.label);
        let expected = match completion.difference {
            Some(_) => ComparisonVerdict::Diff,
            None => ComparisonVerdict::Match,
        };
        let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
        let records = ctx
            .submit(
                &label,
                &crate::session::request(
                    &vendor,
                    Some(&production),
                    None,
                    vec![row],
                    RESORT_EVENTS,
                ),
                Some(expected),
            )?
            .records
            .clone();
        let status = crate::i2c::returned_low(&records, 0, true);
        if status != Some(0) {
            return Err(invalid(format!("{label}: production reported {status:?}")));
        }
        if let Some((reason, mpdu)) = completion.difference {
            let at = records.iter().find_map(|record| match record {
                blobray_domain::ExecutionEvidence::Comparison { result, .. } => {
                    result.difference.clone()
                }
                _ => None,
            });
            let expected_at = blobray_domain::ComparisonDifference::Memory {
                pair: mpdu,
                offset: RETRY_BYTE,
                vendor: FRAME_CONTROL[1] | RETRY,
                replacement: FRAME_CONTROL[1],
            };
            if at.as_ref() != Some(&expected_at) {
                return Err(invalid(format!(
                    "{label}: differs at {at:x?}, not at MPDU {mpdu}'s Retry bit: {reason}"
                )));
            }
        }
        let sent_bar = crate::evidence::events(&records, 0, false).iter().any(|event| {
            matches!(event, blobray_domain::ExecutionEvent::ModeledCall { target, .. } if *target == request_bar)
        });
        if sent_bar != completion.bar_pending {
            return Err(invalid(format!(
                "{label}: the vendor BlockAckReq was {}sent: {BAR_GAP}",
                if sent_bar { "" } else { "not " }
            )));
        }
    }
    Ok(())
}
