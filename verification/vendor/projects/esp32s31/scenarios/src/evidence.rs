//! Strict interpretation of retained execution evidence.
//!
//! Every helper fails instead of guessing: unknown, unavailable or
//! noncontiguous bytes and unknown call words are never read as values.
use crate::layout::{I2C_HOST_MAP, I2C_PORTS, I2C_READ_MASK, RADIO_MMIO, RADIO_MMIO_BYTES};
use blobray_domain::{
    ExecutionEvent, ExecutionEvidence, ExecutionStop, ObservedCallTarget, ObservedWord,
};

/// All selected bytes of the first final-memory selection of one case side.
pub fn output(records: &[ExecutionEvidence], case: u32, side: bool) -> Vec<u8> {
    let chunks: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::FinalMemory {
                case: c,
                replacement,
                chunk,
            } if *c == case && *replacement == side => Some(chunk),
            _ => None,
        })
        .collect();
    assert!(
        !chunks.is_empty(),
        "case {case} side {side} has no final memory"
    );
    let mut offset = 0;
    let mut bytes = vec![];
    for chunk in chunks {
        assert!(
            chunk.selection == 0 && chunk.offset == offset,
            "noncontiguous final memory in case {case}"
        );
        let mask = chunk.mask().expect("valid chunk length");
        assert!(
            chunk.known == mask && chunk.available == mask,
            "unknown or unavailable final memory in case {case}"
        );
        bytes.extend_from_slice(&chunk.bytes[..usize::from(chunk.length)]);
        offset += u32::from(chunk.length);
    }
    bytes
}

/// Executed instruction steps of one case side.
pub fn steps(records: &[ExecutionEvidence], case: u32, side: bool) -> u64 {
    records
        .iter()
        .find_map(|r| match r {
            ExecutionEvidence::Outcome {
                case: c,
                replacement,
                steps,
                ..
            } if *c == case && *replacement == side => Some(*steps),
            _ => None,
        })
        .unwrap_or_else(|| panic!("case {case} side {side} has no outcome"))
}

/// Ordered events of one case side.
pub fn events(records: &[ExecutionEvidence], case: u32, side: bool) -> Vec<ExecutionEvent> {
    records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                case: c,
                replacement,
                event,
            } if *c == case && *replacement == side => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Terminal stop of one case side.
pub fn stop(records: &[ExecutionEvidence], case: u32, side: bool) -> ExecutionStop {
    records
        .iter()
        .find_map(|r| match r {
            ExecutionEvidence::Outcome {
                case: c,
                replacement,
                stop,
                ..
            } if *c == case && *replacement == side => Some(stop.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("case {case} side {side} has no outcome"))
}

pub fn outcomes(records: &[ExecutionEvidence]) -> Vec<&ExecutionStop> {
    records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Outcome { stop, .. } => Some(stop),
            _ => None,
        })
        .collect()
}

pub fn has_events(records: &[ExecutionEvidence]) -> bool {
    records
        .iter()
        .any(|r| matches!(r, ExecutionEvidence::Event { .. }))
}

/// Known argument words of each captured-code transfer to `target`.
pub fn calls(observations: &[ExecutionEvent], target: u32) -> Vec<Vec<u32>> {
    let mut found = vec![];
    for (index, event) in observations.iter().enumerate() {
        let ExecutionEvent::CallTransfer {
            target: t,
            target_kind,
            words,
            ..
        } = event
        else {
            continue;
        };
        if *t != target {
            continue;
        }
        assert_eq!(
            *target_kind,
            ObservedCallTarget::CapturedCode,
            "call to {target:#x} is not captured code"
        );
        let arguments = &observations[index + 1..];
        let words = usize::from(*words);
        assert!(arguments.len() >= words, "missing call argument records");
        found.push(
            arguments[..words]
                .iter()
                .map(|a| match a {
                    ExecutionEvent::TransferArgument {
                        value: ObservedWord::Known { value },
                        ..
                    } => *value,
                    other => panic!("call argument is not a known word: {other:?}"),
                })
                .collect(),
        );
    }
    found
}

/// Kind, address and value of an MMIO transaction, for ordered comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Read,
    Write,
}
pub type Effect = (Access, u32, u32);

/// MMIO read/write effects, all of which must be word-sized. Other event kinds
/// in `observed` must be listed in `ignored` or the helper fails.
pub fn effects(observed: &[ExecutionEvent], ignore_calls: bool) -> Vec<Effect> {
    observed
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Read {
                address,
                width,
                value,
            } => {
                assert_eq!(*width, 4, "non-word MMIO read");
                Some((Access::Read, *address, *value))
            }
            ExecutionEvent::Write {
                address,
                width,
                value,
            } => {
                assert_eq!(*width, 4, "non-word MMIO write");
                Some((Access::Write, *address, *value))
            }
            ExecutionEvent::CallTransfer { .. } | ExecutionEvent::TransferArgument { .. }
                if ignore_calls =>
            {
                None
            }
            other => panic!("unexpected event {other:?}"),
        })
        .collect()
}

/// Radio registers whose first access in the phase is a read, excluding the
/// transport: the values the environment supplied rather than the code wrote.
/// Both sides of a comparison must consume the same environment.
pub fn environment_reads(observed: &[ExecutionEvent]) -> std::collections::BTreeSet<u32> {
    let mut written = std::collections::BTreeSet::new();
    let mut read = std::collections::BTreeSet::new();
    for event in observed {
        match event {
            ExecutionEvent::Read { address, .. }
                if (RADIO_MMIO..RADIO_MMIO + RADIO_MMIO_BYTES).contains(address)
                    && !is_transport(*address)
                    && !written.contains(address) =>
            {
                read.insert(*address);
            }
            ExecutionEvent::Write { address, .. } => {
                written.insert(*address);
            }
            _ => {}
        }
    }
    read
}

/// One ordered PHY effect of a runner-side comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyEffect {
    Read(u32, u32),
    Write(u32, u32),
    Delay(u32),
}

/// Analog I2C transport registers whose reads are command polling.
fn is_transport(address: u32) -> bool {
    I2C_PORTS.contains(&address) || matches!(address, I2C_READ_MASK | I2C_HOST_MAP)
}

/// Ordered word MMIO effects and requested delays, without transport
/// plumbing: transport reads, read-mask and host-map writes, and the
/// single-microsecond delay immediately before a transport read or a read of
/// one of `wait_status`. Every other delay, including readiness waits, stays
/// visible. Non-word MMIO fails.
pub fn phy_effects(observed: &[ExecutionEvent], wait_status: &[u32]) -> Vec<PhyEffect> {
    let ordered: Vec<PhyEffect> = observed
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Read {
                address,
                width: 4,
                value,
            } => Some(PhyEffect::Read(*address, *value)),
            ExecutionEvent::Write {
                address,
                width: 4,
                value,
            } => Some(PhyEffect::Write(*address, *value)),
            ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. } => {
                panic!("non-word PHY MMIO {event:?}")
            }
            ExecutionEvent::DelayMicros { value } => Some(PhyEffect::Delay(*value)),
            _ => None,
        })
        .collect();
    ordered
        .iter()
        .enumerate()
        .filter(|(i, effect)| match effect {
            PhyEffect::Read(address, _) => !is_transport(*address),
            PhyEffect::Write(address, _) => !matches!(*address, I2C_READ_MASK | I2C_HOST_MAP),
            PhyEffect::Delay(1) => !matches!(
                ordered.get(i + 1),
                Some(PhyEffect::Read(address, _))
                    if is_transport(*address) || wait_status.contains(address)
            ),
            PhyEffect::Delay(_) => true,
        })
        .map(|(_, effect)| *effect)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::*;
    use blobray_domain::FinalMemoryChunk;

    fn chunk(offset: u32, known: u16, available: u16, selection: u16) -> ExecutionEvidence {
        let mut bytes = [0; 16];
        bytes[..2].copy_from_slice(&[0x12, 0x34]);
        ExecutionEvidence::FinalMemory {
            case: 1,
            replacement: false,
            chunk: FinalMemoryChunk {
                selection,
                offset,
                length: 2,
                bytes,
                available,
                known,
            },
        }
    }

    #[test]
    fn output_reads_known_contiguous_bytes() {
        assert_eq!(output(&[chunk(0, 3, 3, 0)], 1, false), [0x12, 0x34]);
    }

    #[test]
    fn output_rejects_unknown_unavailable_and_noncontiguous_bytes() {
        for record in [
            chunk(0, 1, 3, 0),
            chunk(0, 3, 1, 0),
            chunk(1, 3, 3, 0),
            chunk(0, 3, 3, 1),
        ] {
            let result = std::panic::catch_unwind(|| output(&[record], 1, false));
            assert!(result.is_err());
        }
        assert!(
            std::panic::catch_unwind(|| output(&[chunk(0, 3, 3, 0), chunk(0, 3, 3, 0)], 1, false))
                .is_err()
        );
        assert!(std::panic::catch_unwind(|| output(&[chunk(0, 3, 3, 0)], 2, false)).is_err());
    }

    #[test]
    fn phy_effects_drop_transport_plumbing_only() {
        use crate::layout::I2C_PORT_0;
        let read = |address, value| ExecutionEvent::Read {
            address,
            width: 4,
            value,
        };
        let write = |address, value| ExecutionEvent::Write {
            address,
            width: 4,
            value,
        };
        let delay = |value| ExecutionEvent::DelayMicros { value };
        let observed = [
            write(I2C_READ_MASK, 1),
            write(I2C_PORT_0, 0x0400_0669),
            delay(1),
            read(I2C_PORT_0, 0x0455_0669),
            read(TEMPERATURE_CODE, 100),
            delay(1),
            read(CHANNEL_STATUS, 0x100),
            delay(1),
            read(PBUS_STATUS, 0),
            delay(10),
            read(I2C_HOST_MAP, 0),
        ];
        assert_eq!(
            phy_effects(&observed, &[PBUS_STATUS]),
            [
                PhyEffect::Write(I2C_PORT_0, 0x0400_0669),
                PhyEffect::Read(TEMPERATURE_CODE, 100),
                PhyEffect::Delay(1),
                PhyEffect::Read(CHANNEL_STATUS, 0x100),
                PhyEffect::Read(PBUS_STATUS, 0),
                PhyEffect::Delay(10),
            ]
        );
        // Without a declared status, its readiness wait stays visible.
        assert!(phy_effects(&observed, &[]).len() == 7);
    }

    fn transfer(kind: ObservedCallTarget) -> ExecutionEvent {
        ExecutionEvent::CallTransfer {
            site: 0,
            target: 10,
            tail: false,
            indirect: false,
            stack: None,
            target_kind: kind,
            words: 1,
        }
    }

    #[test]
    fn calls_reject_models_and_unknown_arguments() {
        let known = ExecutionEvent::TransferArgument {
            word: 0,
            value: ObservedWord::Known { value: 123 },
        };
        assert_eq!(
            calls(
                &[transfer(ObservedCallTarget::CapturedCode), known.clone()],
                10
            ),
            [[123]]
        );
        let unknown = ExecutionEvent::TransferArgument {
            word: 0,
            value: ObservedWord::Unknown,
        };
        for events in [
            vec![transfer(ObservedCallTarget::CallModel), known],
            vec![transfer(ObservedCallTarget::CapturedCode), unknown],
            vec![transfer(ObservedCallTarget::CapturedCode)],
        ] {
            assert!(std::panic::catch_unwind(|| calls(&events, 10)).is_err());
        }
    }
}
