use std::collections::{BTreeMap, BTreeSet};

use crate::artifacts;

use super::{FlowArgumentEvidence, FlowPointeeEvidence};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ValueDomain {
    Constants(BTreeSet<u32>),
    RootArgument(usize),
    Symbolic(String),
    Unknown,
}

impl ValueDomain {
    pub(super) fn render(&self) -> String {
        match self {
            Self::Constants(values) if values.len() == 1 => {
                format!("{:#010x}", values.first().expect("one value"))
            }
            Self::Constants(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|value| format!("{value:#010x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::RootArgument(position) => format!("root.arg{position}"),
            Self::Symbolic(value) => value.clone(),
            Self::Unknown => "unknown".to_owned(),
        }
    }

    fn provenance(&self) -> &'static str {
        match self {
            Self::Constants(_) => "exact-constant-domain",
            Self::RootArgument(_) => "root-argument",
            Self::Symbolic(_) => "uncomposed-symbolic-expression",
            Self::Unknown => "unknown",
        }
    }

    fn constants(&self) -> Vec<u32> {
        match self {
            Self::Constants(values) => values.iter().copied().collect(),
            _ => Vec::new(),
        }
    }
}

pub(super) fn root_domains() -> Vec<ValueDomain> {
    (0..16).map(ValueDomain::RootArgument).collect()
}

pub(super) fn compose_call_arguments(
    function: &artifacts::StoredFunction,
    call: &artifacts::StoredCall,
    domains: &[ValueDomain],
) -> (Vec<ValueDomain>, Vec<FlowArgumentEvidence>) {
    let next = call
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            if call.argument_is_exact(position) {
                resolve_value(value, domains)
            } else {
                ValueDomain::Unknown
            }
        })
        .collect::<Vec<_>>();
    let arguments = call
        .arguments
        .iter()
        .zip(&next)
        .enumerate()
        .map(|(position, (local, resolved))| FlowArgumentEvidence {
            position,
            local: local.clone(),
            resolved: resolved.render(),
            constants: resolved.constants(),
            provenance: resolved.provenance(),
            pointee: stack_pointee(function, call.site, local, domains),
        })
        .collect();
    (next, arguments)
}

fn resolve_value(value: &str, caller: &[ValueDomain]) -> ValueDomain {
    if let Some(value) = value.strip_prefix("const:") {
        return parse_u32(value)
            .map(|value| ValueDomain::Constants(BTreeSet::from([value])))
            .unwrap_or(ValueDomain::Unknown);
    }
    if let Some(values) = value
        .strip_prefix("one-of(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let constants = values
            .split(',')
            .filter_map(parse_u32)
            .collect::<BTreeSet<_>>();
        return if constants.is_empty() {
            ValueDomain::Unknown
        } else {
            ValueDomain::Constants(constants)
        };
    }
    if let Some(position) = value
        .strip_prefix("arg")
        .and_then(|value| value.parse::<usize>().ok())
    {
        return caller
            .get(position)
            .cloned()
            .unwrap_or(ValueDomain::Unknown);
    }
    if value == "unknown" || value.starts_with("varies-across-") {
        ValueDomain::Unknown
    } else {
        ValueDomain::Symbolic(value.to_owned())
    }
}

fn stack_pointee(
    function: &artifacts::StoredFunction,
    call_site: Option<u32>,
    pointer: &str,
    caller: &[ValueDomain],
) -> Vec<FlowPointeeEvidence> {
    let Some(base) = parse_stack_offset(pointer) else {
        return Vec::new();
    };
    let mut latest = BTreeMap::<(i32, u8), (u32, &artifacts::StoredFlowValue)>::new();
    for fact in &function.local_value_flow {
        let artifacts::StoredLocalValueFlow::StackStore {
            site,
            offset,
            width,
            value,
        } = fact
        else {
            continue;
        };
        if call_site.is_some_and(|call_site| *site >= call_site) {
            continue;
        }
        let relative = offset.wrapping_sub(base);
        if !(0..=256).contains(&relative) {
            continue;
        }
        let key = (relative, *width);
        if latest.get(&key).is_none_or(|(previous, _)| previous < site) {
            latest.insert(key, (*site, value));
        }
    }
    latest
        .into_iter()
        .map(|((offset, width), (_, value))| {
            let resolved = value.constant.map_or_else(
                || {
                    value
                        .input
                        .and_then(|position| caller.get(usize::from(position)).cloned())
                        .unwrap_or_else(|| resolve_value(&value.expression, caller))
                },
                |value| ValueDomain::Constants(BTreeSet::from([value])),
            );
            FlowPointeeEvidence {
                offset,
                width,
                local: value.expression.clone(),
                resolved: resolved.render(),
                constants: resolved.constants(),
                provenance: resolved.provenance(),
            }
        })
        .collect()
}

fn parse_stack_offset(value: &str) -> Option<i32> {
    let value = value.strip_prefix("private-stack:")?;
    let value = value.strip_prefix('+').unwrap_or(value);
    if let Some(hex) = value.strip_prefix("-0x") {
        return i32::try_from(u32::from_str_radix(hex, 16).ok()?)
            .ok()
            .and_then(i32::checked_neg);
    }
    value
        .strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .map(|value| value as i32)
        .or_else(|| value.parse().ok())
}

fn parse_u32(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .or_else(|| value.parse().ok())
}
