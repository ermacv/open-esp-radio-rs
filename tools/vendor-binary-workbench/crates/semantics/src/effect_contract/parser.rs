//! Fail-closed parser for effect-policy rules.

use super::*;

fn parse_width(value: &str, line: usize) -> Result<u8> {
    let width = value
        .parse::<u8>()
        .map_err(|_| format!("invalid effect width {value:?} at line {line}"))?;
    if matches!(width, 8 | 16 | 32) {
        Ok(width)
    } else {
        Err(format!("unsupported effect width {width} at line {line}").into())
    }
}

fn parse_state_field(value: &str, line: usize) -> Result<String> {
    let Some((projection, field)) = value.split_once('.') else {
        return Err(format!(
            "state effect requires PROJECTION.FIELD, received {value:?} at line {line}"
        )
        .into());
    };
    if projection.is_empty() || field.is_empty() || field.contains('.') {
        return Err(format!("invalid state field {value:?} at line {line}").into());
    }
    Ok(value.to_owned())
}

fn parse_boundary_id(value: &str, kind: &str, line: usize) -> Result<String> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!("invalid {kind} id {value:?} at line {line}").into());
    }
    Ok(value.to_owned())
}

pub fn parse_effect_rule(value: &str, line: usize) -> Result<(EffectSelector, EffectDisposition)> {
    let mut words = value.split_whitespace();
    let kind = words
        .next()
        .ok_or_else(|| format!("effect has no kind at line {line}"))?;
    let selector = match kind {
        "mmio-read" | "mmio-write" => {
            let width = parse_width(
                words
                    .next()
                    .ok_or_else(|| format!("{kind} has no width at line {line}"))?,
                line,
            )?;
            let address_text = words
                .next()
                .ok_or_else(|| format!("{kind} has no address at line {line}"))?;
            let address = u32_literal(address_text)
                .ok_or_else(|| format!("invalid effect address {address_text:?} at line {line}"))?;
            if kind == "mmio-read" {
                EffectSelector::MmioRead { width, address }
            } else {
                EffectSelector::MmioWrite { width, address }
            }
        }
        "state-read" | "state-write" => {
            let width = parse_width(
                words
                    .next()
                    .ok_or_else(|| format!("{kind} has no width at line {line}"))?,
                line,
            )?;
            let field = parse_state_field(
                words
                    .next()
                    .ok_or_else(|| format!("{kind} has no field at line {line}"))?,
                line,
            )?;
            if kind == "state-read" {
                EffectSelector::StateRead { width, field }
            } else {
                EffectSelector::StateWrite { width, field }
            }
        }
        "delay" => EffectSelector::Delay,
        "await-ready" => EffectSelector::AwaitReady {
            condition: words
                .next()
                .filter(|condition| !condition.is_empty())
                .ok_or_else(|| format!("await-ready has no condition at line {line}"))?
                .to_owned(),
        },
        "platform-call" => EffectSelector::PlatformCall {
            operation: PlatformOperation::parse(
                words
                    .next()
                    .ok_or_else(|| format!("platform-call has no operation at line {line}"))?,
                line,
            )?,
        },
        "platform-provided-input" => EffectSelector::PlatformProvidedInput {
            input: parse_boundary_id(
                words.next().ok_or_else(|| {
                    format!("platform-provided-input has no input id at line {line}")
                })?,
                "platform-provided-input",
                line,
            )?,
        },
        "platform-provided-service" => EffectSelector::PlatformProvidedService {
            service: parse_boundary_id(
                words.next().ok_or_else(|| {
                    format!("platform-provided-service has no service id at line {line}")
                })?,
                "platform-provided-service",
                line,
            )?,
        },
        "published-event" => EffectSelector::PublishedEvent {
            event: parse_boundary_id(
                words
                    .next()
                    .ok_or_else(|| format!("published-event has no event id at line {line}"))?,
                "published-event",
                line,
            )?,
        },
        "initialization-prerequisite" => EffectSelector::InitializationPrerequisite {
            prerequisite: parse_boundary_id(
                words.next().ok_or_else(|| {
                    format!("initialization-prerequisite has no prerequisite id at line {line}")
                })?,
                "initialization-prerequisite",
                line,
            )?,
        },
        _ => return Err(format!("unknown effect kind {kind:?} at line {line}").into()),
    };
    let disposition_name = words
        .next()
        .ok_or_else(|| format!("effect has no disposition at line {line}"))?;
    let disposition = match disposition_name {
        "required" => EffectDisposition::Required,
        "replaced-by-async" => EffectDisposition::ReplacedByAsync {
            condition: words
                .next()
                .filter(|condition| !condition.is_empty())
                .ok_or_else(|| format!("replaced-by-async has no condition at line {line}"))?
                .to_owned(),
            timeout: Timeout::parse(
                words
                    .next()
                    .ok_or_else(|| format!("replaced-by-async has no timeout at line {line}"))?,
                line,
            )?,
        },
        "platform-provided-input" => EffectDisposition::PlatformProvidedInput {
            input: parse_boundary_id(
                words.next().ok_or_else(|| {
                    format!("platform-provided-input has no input id at line {line}")
                })?,
                "platform-provided-input",
                line,
            )?,
        },
        "platform-provided-service" => EffectDisposition::PlatformProvidedService {
            service: parse_boundary_id(
                words.next().ok_or_else(|| {
                    format!("platform-provided-service has no service id at line {line}")
                })?,
                "platform-provided-service",
                line,
            )?,
        },
        "published-event" => EffectDisposition::PublishedEvent {
            event: parse_boundary_id(
                words
                    .next()
                    .ok_or_else(|| format!("published-event has no event id at line {line}"))?,
                "published-event",
                line,
            )?,
        },
        "initialization-prerequisite" => EffectDisposition::InitializationPrerequisite {
            prerequisite: parse_boundary_id(
                words.next().ok_or_else(|| {
                    format!("initialization-prerequisite has no prerequisite id at line {line}")
                })?,
                "initialization-prerequisite",
                line,
            )?,
        },
        "platform-owned" => EffectDisposition::PlatformOwned,
        "forbidden" => EffectDisposition::Forbidden,
        "allowed-omission" => EffectDisposition::AllowedOmission(OmissionReason::parse(
            words
                .next()
                .ok_or_else(|| format!("allowed-omission has no reason at line {line}"))?,
            line,
        )?),
        _ => {
            return Err(
                format!("unknown effect disposition {disposition_name:?} at line {line}").into(),
            );
        }
    };
    if words.next().is_some() {
        return Err(format!("effect has extra fields at line {line}").into());
    }
    match (&selector, &disposition) {
        (EffectSelector::PlatformCall { .. }, EffectDisposition::AllowedOmission(_))
        | (EffectSelector::PlatformCall { .. }, EffectDisposition::PlatformOwned)
        | (_, EffectDisposition::Required | EffectDisposition::Forbidden)
        | (
            EffectSelector::Delay | EffectSelector::MmioRead { .. },
            EffectDisposition::ReplacedByAsync { .. },
        ) => {}
        (
            EffectSelector::MmioRead { .. }
            | EffectSelector::StateRead { .. }
            | EffectSelector::PlatformCall { .. },
            EffectDisposition::PlatformProvidedInput { .. },
        )
        | (
            EffectSelector::PlatformCall { .. },
            EffectDisposition::PlatformProvidedService { .. },
        )
        | (
            EffectSelector::StateWrite { .. } | EffectSelector::PlatformCall { .. },
            EffectDisposition::PublishedEvent { .. },
        )
        | (_, EffectDisposition::InitializationPrerequisite { .. }) => {}
        (_, EffectDisposition::AllowedOmission(_)) => {
            return Err(format!(
                "allowed-omission applies only to platform-call effects at line {line}"
            )
            .into());
        }
        (_, EffectDisposition::PlatformOwned) => {
            return Err(format!(
                "platform-owned applies only to platform-call effects at line {line}"
            )
            .into());
        }
        (_, EffectDisposition::ReplacedByAsync { .. }) => {
            return Err(format!(
                "replaced-by-async applies only to delay or MMIO-read effects at line {line}"
            )
            .into());
        }
        (_, EffectDisposition::PlatformProvidedInput { .. }) => {
            return Err(format!(
                "platform-provided-input applies only to read or platform-call effects at line {line}"
            )
            .into());
        }
        (_, EffectDisposition::PlatformProvidedService { .. }) => {
            return Err(format!(
                "platform-provided-service applies only to platform-call effects at line {line}"
            )
            .into());
        }
        (_, EffectDisposition::PublishedEvent { .. }) => {
            return Err(format!(
                "published-event applies only to state-write or platform-call effects at line {line}"
            )
            .into());
        }
    }
    Ok((selector, disposition))
}
