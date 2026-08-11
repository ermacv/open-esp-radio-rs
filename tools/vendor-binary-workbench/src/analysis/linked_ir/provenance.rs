//! Return-bit provenance and guard-to-MMIO linkage.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReturnBitDescriptor {
    kind: &'static str,
    source_bit: u8,
    inverted: bool,
    argument: Option<u8>,
    token: Option<u32>,
    target: Option<String>,
    address: Option<u32>,
    register: Option<String>,
}

impl ReturnBitDescriptor {
    fn continues_with(&self, next: &Self) -> bool {
        self.kind == next.kind
            && self.source_bit.checked_add(1) == Some(next.source_bit)
            && self.inverted == next.inverted
            && self.argument == next.argument
            && self.token == next.token
            && self.target == next.target
            && self.address == next.address
            && self.register == next.register
    }
}

fn return_bit_descriptor(
    source: BitSource,
    call_results: &BTreeMap<u32, String>,
    svd: &MmioMap,
) -> Option<ReturnBitDescriptor> {
    let descriptor = match source {
        BitSource::Input {
            index,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "argument",
            source_bit: bit,
            inverted,
            argument: Some(index),
            token: None,
            target: None,
            address: None,
            register: None,
        },
        BitSource::Register {
            read_token,
            address,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "mmio-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: Some(address),
            register: Some(svd.display_register_name(address)),
        },
        BitSource::IndexedRegister {
            read_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "indexed-mmio-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: None,
            register: None,
        },
        BitSource::Memory {
            read_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "memory-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: None,
            register: None,
        },
        BitSource::PrivateStack {
            read_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "private-stack-read",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(read_token),
            target: None,
            address: None,
            register: None,
        },
        BitSource::CallResult {
            call_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "call-result",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(call_token),
            target: call_results.get(&call_token).cloned(),
            address: None,
            register: None,
        },
        BitSource::ExternalResult {
            call_token,
            bit,
            inverted,
        } => {
            let call_token = external_result_call_token(call_token);
            ReturnBitDescriptor {
                kind: "external-result",
                source_bit: bit,
                inverted,
                argument: None,
                token: Some(call_token),
                target: call_results.get(&call_token).cloned(),
                address: None,
                register: None,
            }
        }
        BitSource::ExternalResultHigh {
            call_token,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "external-result-high",
            source_bit: bit,
            inverted,
            argument: None,
            token: Some(call_token),
            target: call_results.get(&call_token).cloned(),
            address: None,
            register: None,
        },
        BitSource::ExternalOutput {
            call_token,
            output_index,
            bit,
            inverted,
        } => ReturnBitDescriptor {
            kind: "external-output",
            source_bit: bit,
            inverted,
            argument: Some(output_index),
            token: Some(call_token),
            target: call_results.get(&call_token).cloned(),
            address: None,
            register: None,
        },
        BitSource::Unknown | BitSource::Constant(_) => return None,
    };
    Some(descriptor)
}

pub(super) fn return_provenance(
    value: &SymbolicValue,
    call_results: &BTreeMap<u32, String>,
    svd: &MmioMap,
) -> LinkedReturnProvenance {
    let bits = value.bits();
    let mut known_zero_bits = 0_u32;
    let mut known_one_bits = 0_u32;
    let mut unknown_bits = 0_u32;
    let mut sources = Vec::new();
    let mut output_bit = 0_usize;
    while output_bit < bits.len() {
        match bits[output_bit] {
            BitSource::Constant(false) => {
                known_zero_bits |= 1_u32 << output_bit;
                output_bit += 1;
            }
            BitSource::Constant(true) => {
                known_one_bits |= 1_u32 << output_bit;
                output_bit += 1;
            }
            BitSource::Unknown => {
                unknown_bits |= 1_u32 << output_bit;
                output_bit += 1;
            }
            source => {
                let descriptor = return_bit_descriptor(source, call_results, svd)
                    .expect("non-constant return bit has a source descriptor");
                let first_output_bit = output_bit;
                let first_source_bit = descriptor.source_bit;
                let mut previous = descriptor.clone();
                output_bit += 1;
                while output_bit < bits.len() {
                    let Some(next) = return_bit_descriptor(bits[output_bit], call_results, svd)
                    else {
                        break;
                    };
                    if !previous.continues_with(&next) {
                        break;
                    }
                    previous = next;
                    output_bit += 1;
                }
                let width = (output_bit - first_output_bit) as u8;
                sources.push(LinkedReturnBitSource {
                    kind: descriptor.kind,
                    output_lsb: first_output_bit as u8,
                    source_lsb: first_source_bit,
                    width,
                    output_bits: bit_range_mask(first_output_bit as u8, width),
                    source_bits: bit_range_mask(first_source_bit, width),
                    inverted: descriptor.inverted,
                    argument: descriptor.argument,
                    token: descriptor.token,
                    target: descriptor.target,
                    address: descriptor.address,
                    register: descriptor.register,
                });
            }
        }
    }
    LinkedReturnProvenance {
        exact: unknown_bits == 0,
        known_zero_bits,
        known_one_bits,
        unknown_bits,
        sources,
    }
}

pub(super) fn trace_call_results(
    trace: &FunctionAnalysis,
    identities: &IrIdentityCatalog,
) -> BTreeMap<u32, String> {
    let mut candidates = BTreeMap::<u32, BTreeSet<String>>::new();
    for event in &trace.reference_events {
        if let Some((token, target)) = call_result_identity(event, identities) {
            candidates.entry(token).or_default().insert(target);
        }
    }
    candidates
        .into_iter()
        .filter_map(|(token, targets)| {
            let mut targets = targets.into_iter();
            let target = targets.next()?;
            targets.next().is_none().then_some((token, target))
        })
        .collect()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ProjectedMmioReturnBit {
    address: u32,
    register: String,
    register_bit: u8,
    inverted: bool,
    producer_path: Vec<String>,
}

fn project_return_bit_to_mmio(
    producer: &str,
    output_bit: u8,
    producers: &BTreeMap<String, LinkedReturnProvenance>,
) -> Option<ProjectedMmioReturnBit> {
    let mut producer = producer.to_owned();
    let mut output_bit = output_bit;
    let mut active = BTreeSet::new();
    let mut producer_path = Vec::new();
    let mut inverted = false;
    loop {
        if !active.insert((producer.clone(), output_bit)) {
            return None;
        }
        producer_path.push(producer.clone());
        let provenance = producers.get(&producer)?;
        let source = provenance
            .sources
            .iter()
            .find(|source| source.output_bits & (1_u32 << output_bit) != 0)?;
        let source_bit = source.source_lsb + (output_bit - source.output_lsb);
        inverted ^= source.inverted;
        match source.kind {
            "mmio-read" => {
                return Some(ProjectedMmioReturnBit {
                    address: source
                        .address
                        .expect("MMIO return source has a concrete address"),
                    register: source
                        .register
                        .clone()
                        .expect("MMIO return source has a register label"),
                    register_bit: source_bit,
                    inverted,
                    producer_path,
                });
            }
            "call-result" => {
                producer = source.target.clone()?;
                output_bit = source_bit;
            }
            _ => return None,
        }
    }
}

#[derive(Default)]
struct GuardMmioSourceAccumulator {
    result_bits: u32,
    register_bits: u32,
    comparison_known_bits: u32,
    comparison_one_bits: u32,
    comparison_conflict: bool,
}

pub(super) fn guard_mmio_sources(
    result_source: &LinkedCallGuardResultSource,
    producer: &str,
    producers: &BTreeMap<String, LinkedReturnProvenance>,
) -> Vec<LinkedCallGuardMmioSource> {
    let mut sources =
        BTreeMap::<(u32, String, bool, Vec<String>), GuardMmioSourceAccumulator>::new();
    for result_bit in 0..32_u8 {
        if result_source.source_bits & (1_u32 << result_bit) == 0 {
            continue;
        }
        let Some(projected) = project_return_bit_to_mmio(producer, result_bit, producers) else {
            continue;
        };
        let register_mask = 1_u32 << projected.register_bit;
        let entry = sources
            .entry((
                projected.address,
                projected.register,
                result_source.inverted ^ projected.inverted,
                projected.producer_path,
            ))
            .or_default();
        entry.result_bits |= 1_u32 << result_bit;
        entry.register_bits |= register_mask;
        let Some(comparison_value) = result_source.source_comparison_value else {
            continue;
        };
        let expected = (comparison_value & (1_u32 << result_bit) != 0) ^ projected.inverted;
        if entry.comparison_known_bits & register_mask != 0 {
            let previous = entry.comparison_one_bits & register_mask != 0;
            entry.comparison_conflict |= previous != expected;
        } else {
            entry.comparison_known_bits |= register_mask;
            if expected {
                entry.comparison_one_bits |= register_mask;
            }
        }
    }
    sources
        .into_iter()
        .map(
            |((address, register, inverted, producer_path), evidence)| LinkedCallGuardMmioSource {
                address,
                register,
                producer_path,
                result_bits: evidence.result_bits,
                register_bits: evidence.register_bits,
                inverted,
                result_comparison_value: result_source
                    .source_comparison_value
                    .map(|value| value & evidence.result_bits),
                register_comparison_value: result_source
                    .source_comparison_value
                    .filter(|_| {
                        !evidence.comparison_conflict
                            && evidence.comparison_known_bits == evidence.register_bits
                    })
                    .map(|_| evidence.comparison_one_bits),
            },
        )
        .collect()
}

pub(super) fn link_guard_result_mmio_sources(functions: &mut [LinkedIrFunction]) {
    let producers = functions
        .iter()
        .map(|function| {
            (
                function.identity.clone(),
                function.return_provenance.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for source in functions
        .iter_mut()
        .flat_map(|function| &mut function.calls)
        .filter_map(|call| call.guard_paths.as_mut())
        .flatten()
        .flat_map(|path| &mut path.guards)
        .flat_map(|guard| &mut guard.result_sources)
    {
        let Some(target) = source.target.as_deref() else {
            continue;
        };
        let Some(provenance) = producers.get(target) else {
            continue;
        };
        source.producer_return_exact = Some(provenance.exact);
        source.mmio_sources = guard_mmio_sources(source, target, &producers);
    }
}
