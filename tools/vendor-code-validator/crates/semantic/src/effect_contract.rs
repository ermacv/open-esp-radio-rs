//! Canonical observable-effect contract and fail-closed comparison policy.

use std::collections::{BTreeMap, BTreeSet};

use crate::{MemoryAccess, ObservableEvent, Result, SymbolicValue, u32_literal};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RegisterId {
    pub address: u32,
    pub width: u8,
    pub name: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StateField {
    pub projection: String,
    pub field: String,
    pub width: u8,
}

impl StateField {
    fn id(&self) -> String {
        format!("{}.{}", self.projection, self.field)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "Effect Contract v1 defines concrete, state, and async values before all pilots consume them"
)]
pub enum ContractValue {
    Concrete(u32),
    ReadResult { ordinal: u32 },
    Symbolic(SymbolicValue),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PlatformOperation {
    DebugDiagnostic,
    NvsCalibrationCache,
    RtosSchedulingAdapter,
    DelayMicros,
    Random,
    CriticalSection,
    Allocate,
    Deallocate,
}

impl PlatformOperation {
    pub fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "debug-diagnostic" => Ok(Self::DebugDiagnostic),
            "nvs-calibration-cache" => Ok(Self::NvsCalibrationCache),
            "rtos-scheduling-adapter" => Ok(Self::RtosSchedulingAdapter),
            "delay-micros" => Ok(Self::DelayMicros),
            "random" => Ok(Self::Random),
            "critical-section" => Ok(Self::CriticalSection),
            "allocate" => Ok(Self::Allocate),
            "deallocate" => Ok(Self::Deallocate),
            _ => Err(format!("unknown platform operation {value:?} at line {line}").into()),
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Self::DebugDiagnostic => "debug-diagnostic",
            Self::NvsCalibrationCache => "nvs-calibration-cache",
            Self::RtosSchedulingAdapter => "rtos-scheduling-adapter",
            Self::DelayMicros => "delay-micros",
            Self::Random => "random",
            Self::CriticalSection => "critical-section",
            Self::Allocate => "allocate",
            Self::Deallocate => "deallocate",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "named and MMIO readiness are both part of the v1 async contract"
)]
pub enum ReadyCondition {
    Named(String),
    Mmio {
        register: RegisterId,
        mask: u32,
        expected: u32,
    },
}

impl ReadyCondition {
    pub fn id(&self) -> String {
        match self {
            Self::Named(name) => name.clone(),
            Self::Mmio {
                register,
                mask,
                expected,
            } => format!(
                "mmio:{}@{:#010x}&{mask:#010x}=={expected:#010x}",
                register.width, register.address
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[allow(
    dead_code,
    reason = "attempt and deadline timeouts are both part of the v1 async contract"
)]
pub enum Timeout {
    Attempts(u32),
    DeadlineMicros(u32),
}

impl Timeout {
    fn parse(value: &str, line: usize) -> Result<Self> {
        let (kind, count) = value.split_once('=').ok_or_else(|| {
            format!("async replacement timeout must be KIND=COUNT at line {line}")
        })?;
        let count = u32_literal(count)
            .ok_or_else(|| format!("invalid async replacement timeout {value:?} at line {line}"))?;
        if count == 0 {
            return Err(
                format!("async replacement timeout must be non-zero at line {line}").into(),
            );
        }
        match kind {
            "attempts" => Ok(Self::Attempts(count)),
            "deadline-us" => Ok(Self::DeadlineMicros(count)),
            _ => Err(format!("unknown async replacement timeout {kind:?} at line {line}").into()),
        }
    }

    fn canonical(self) -> String {
        match self {
            Self::Attempts(count) => format!("attempts={count}"),
            Self::DeadlineMicros(micros) => format!("deadline-us={micros}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the v1 schema is complete before state and async pilots are connected"
)]
pub enum ContractEffect {
    MmioRead {
        register: RegisterId,
        value: ContractValue,
    },
    MmioWrite {
        register: RegisterId,
        value: ContractValue,
    },
    StateRead {
        field: StateField,
        value: ContractValue,
    },
    StateWrite {
        field: StateField,
        value: ContractValue,
    },
    Delay {
        micros: ContractValue,
    },
    AwaitReady {
        condition: ReadyCondition,
        timeout: Timeout,
    },
    PlatformCall {
        operation: PlatformOperation,
        arguments: Vec<ContractValue>,
    },
    PlatformProvidedInput {
        input: String,
    },
    PlatformProvidedService {
        service: String,
    },
    PublishedEvent {
        event: String,
    },
    InitializationPrerequisite {
        prerequisite: String,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectSelector {
    MmioRead { width: u8, address: u32 },
    MmioWrite { width: u8, address: u32 },
    StateRead { width: u8, field: String },
    StateWrite { width: u8, field: String },
    Delay,
    AwaitReady { condition: String },
    PlatformCall { operation: PlatformOperation },
    PlatformProvidedInput { input: String },
    PlatformProvidedService { service: String },
    PublishedEvent { event: String },
    InitializationPrerequisite { prerequisite: String },
}

impl EffectSelector {
    pub fn canonical(&self) -> String {
        match self {
            Self::MmioRead { width, address } => {
                format!("mmio-read {width} {address:#010x}")
            }
            Self::MmioWrite { width, address } => {
                format!("mmio-write {width} {address:#010x}")
            }
            Self::StateRead { width, field } => format!("state-read {width} {field}"),
            Self::StateWrite { width, field } => format!("state-write {width} {field}"),
            Self::Delay => "delay".to_owned(),
            Self::AwaitReady { condition } => format!("await-ready {condition}"),
            Self::PlatformCall { operation } => {
                format!("platform-call {}", operation.label())
            }
            Self::PlatformProvidedInput { input } => {
                format!("platform-provided-input {input}")
            }
            Self::PlatformProvidedService { service } => {
                format!("platform-provided-service {service}")
            }
            Self::PublishedEvent { event } => format!("published-event {event}"),
            Self::InitializationPrerequisite { prerequisite } => {
                format!("initialization-prerequisite {prerequisite}")
            }
        }
    }
}

impl ContractEffect {
    pub fn selector(&self) -> EffectSelector {
        match self {
            Self::MmioRead { register, .. } => EffectSelector::MmioRead {
                width: register.width,
                address: register.address,
            },
            Self::MmioWrite { register, .. } => EffectSelector::MmioWrite {
                width: register.width,
                address: register.address,
            },
            Self::StateRead { field, .. } => EffectSelector::StateRead {
                width: field.width,
                field: field.id(),
            },
            Self::StateWrite { field, .. } => EffectSelector::StateWrite {
                width: field.width,
                field: field.id(),
            },
            Self::Delay { .. } => EffectSelector::Delay,
            Self::AwaitReady { condition, .. } => EffectSelector::AwaitReady {
                condition: condition.id(),
            },
            Self::PlatformCall { operation, .. } => EffectSelector::PlatformCall {
                operation: operation.clone(),
            },
            Self::PlatformProvidedInput { input } => EffectSelector::PlatformProvidedInput {
                input: input.clone(),
            },
            Self::PlatformProvidedService { service } => EffectSelector::PlatformProvidedService {
                service: service.clone(),
            },
            Self::PublishedEvent { event } => EffectSelector::PublishedEvent {
                event: event.clone(),
            },
            Self::InitializationPrerequisite { prerequisite } => {
                EffectSelector::InitializationPrerequisite {
                    prerequisite: prerequisite.clone(),
                }
            }
        }
    }

    fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::MmioRead {
                    register: left_register,
                    value: left_value,
                },
                Self::MmioRead {
                    register: right_register,
                    value: right_value,
                },
            )
            | (
                Self::MmioWrite {
                    register: left_register,
                    value: left_value,
                },
                Self::MmioWrite {
                    register: right_register,
                    value: right_value,
                },
            ) => {
                left_register.address == right_register.address
                    && left_register.width == right_register.width
                    && left_value == right_value
            }
            (
                Self::StateRead {
                    field: left_field,
                    value: left_value,
                },
                Self::StateRead {
                    field: right_field,
                    value: right_value,
                },
            )
            | (
                Self::StateWrite {
                    field: left_field,
                    value: left_value,
                },
                Self::StateWrite {
                    field: right_field,
                    value: right_value,
                },
            ) => left_field == right_field && left_value == right_value,
            (Self::Delay { micros: left }, Self::Delay { micros: right }) => left == right,
            (
                Self::AwaitReady {
                    condition: left_condition,
                    timeout: left_timeout,
                },
                Self::AwaitReady {
                    condition: right_condition,
                    timeout: right_timeout,
                },
            ) => left_condition == right_condition && left_timeout == right_timeout,
            (
                Self::PlatformCall {
                    operation: left_operation,
                    arguments: left_arguments,
                },
                Self::PlatformCall {
                    operation: right_operation,
                    arguments: right_arguments,
                },
            ) => left_operation == right_operation && left_arguments == right_arguments,
            (
                Self::PlatformProvidedInput { input: left },
                Self::PlatformProvidedInput { input: right },
            ) => left == right,
            (
                Self::PlatformProvidedService { service: left },
                Self::PlatformProvidedService { service: right },
            ) => left == right,
            (Self::PublishedEvent { event: left }, Self::PublishedEvent { event: right }) => {
                left == right
            }
            (
                Self::InitializationPrerequisite { prerequisite: left },
                Self::InitializationPrerequisite {
                    prerequisite: right,
                },
            ) => left == right,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum OmissionReason {
    DebugDiagnostic,
    NvsCalibrationCache,
    RtosSchedulingAdapter,
    UnusedInstrumentation,
}

impl OmissionReason {
    pub fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "debug-diagnostic" => Ok(Self::DebugDiagnostic),
            "nvs-calibration-cache" => Ok(Self::NvsCalibrationCache),
            "rtos-scheduling-adapter" => Ok(Self::RtosSchedulingAdapter),
            "unused-instrumentation" => Ok(Self::UnusedInstrumentation),
            _ => Err(format!("unknown omission reason {value:?} at line {line}").into()),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::DebugDiagnostic => "debug-diagnostic",
            Self::NvsCalibrationCache => "nvs-calibration-cache",
            Self::RtosSchedulingAdapter => "rtos-scheduling-adapter",
            Self::UnusedInstrumentation => "unused-instrumentation",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectDisposition {
    Required,
    ReplacedByAsync { condition: String, timeout: Timeout },
    PlatformProvidedInput { input: String },
    PlatformProvidedService { service: String },
    PublishedEvent { event: String },
    InitializationPrerequisite { prerequisite: String },
    AllowedOmission(OmissionReason),
    PlatformOwned,
    Forbidden,
}

impl EffectDisposition {
    pub fn canonical(&self) -> String {
        match self {
            Self::Required => "required".to_owned(),
            Self::ReplacedByAsync { condition, timeout } => {
                format!("replaced-by-async {condition} {}", timeout.canonical())
            }
            Self::PlatformProvidedInput { input } => {
                format!("platform-provided-input {input}")
            }
            Self::PlatformProvidedService { service } => {
                format!("platform-provided-service {service}")
            }
            Self::PublishedEvent { event } => format!("published-event {event}"),
            Self::InitializationPrerequisite { prerequisite } => {
                format!("initialization-prerequisite {prerequisite}")
            }
            Self::AllowedOmission(reason) => {
                format!("allowed-omission {}", reason.label())
            }
            Self::PlatformOwned => "platform-owned".to_owned(),
            Self::Forbidden => "forbidden".to_owned(),
        }
    }
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectComparison {
    ExactEffectsV1,
}

impl EffectComparison {
    pub fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "exact-effects-v1" => Ok(Self::ExactEffectsV1),
            _ => Err(format!("unknown effect contract {value:?} at line {line}").into()),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ExactEffectsV1 => "exact-effects-v1",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectPolicy {
    pub comparison: EffectComparison,
    rules: BTreeMap<EffectSelector, EffectDisposition>,
}

impl EffectPolicy {
    pub fn new(
        comparison: EffectComparison,
        rules: impl IntoIterator<Item = (EffectSelector, EffectDisposition)>,
    ) -> Result<Self> {
        let mut collected = BTreeMap::new();
        for (selector, disposition) in rules {
            if collected.insert(selector.clone(), disposition).is_some() {
                return Err(format!("duplicate effect rule {}", selector.canonical()).into());
            }
        }
        if collected.is_empty() {
            return Err("effect contract has no effect rules".into());
        }
        Ok(Self {
            comparison,
            rules: collected,
        })
    }

    pub fn canonical(&self) -> String {
        let mut output = format!("effect-contract {}\n", self.comparison.label());
        for (selector, disposition) in &self.rules {
            output.push_str("effect ");
            output.push_str(&selector.canonical());
            output.push(' ');
            output.push_str(&disposition.canonical());
            output.push('\n');
        }
        output
    }

    pub fn rules(&self) -> impl Iterator<Item = (&EffectSelector, &EffectDisposition)> {
        self.rules.iter()
    }

    pub fn disposition(&self, selector: &EffectSelector) -> Option<&EffectDisposition> {
        self.rules.get(selector)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectComparisonVerdict {
    Match,
    Mismatch(String),
}

pub fn effects_from_observable(events: &[ObservableEvent]) -> Result<Vec<ContractEffect>> {
    let mut effects = Vec::with_capacity(events.len());
    for event in events {
        match event {
            ObservableEvent::Memory {
                access,
                width,
                address,
                register,
                value,
            } => {
                if register == "UNMAPPED" {
                    return Err(format!(
                        "cannot create an effect contract for unmapped MMIO {address:#010x}"
                    )
                    .into());
                }
                let register = RegisterId {
                    address: *address,
                    width: *width,
                    name: register.clone(),
                };
                let effect = match access {
                    MemoryAccess::Read => ContractEffect::MmioRead {
                        register,
                        value: ContractValue::ReadResult {
                            ordinal: effects.len() as u32,
                        },
                    },
                    MemoryAccess::Write => ContractEffect::MmioWrite {
                        register,
                        value: ContractValue::Symbolic(value.clone().ok_or_else(|| {
                            format!("MMIO write at {address:#010x} has no modeled value")
                        })?),
                    },
                };
                effects.push(effect);
            }
            ObservableEvent::Fence { .. } => {
                return Err("memory fence has no Effect Contract v1 classification".into());
            }
        }
    }
    Ok(effects)
}

pub fn compare_effects(
    vendor: &[ContractEffect],
    rust: &[ContractEffect],
    policy: &EffectPolicy,
) -> Result<EffectComparisonVerdict> {
    let mut rust_index = 0_usize;
    let mut used_rules = BTreeSet::new();
    for (vendor_index, vendor_effect) in vendor.iter().enumerate() {
        let selector = vendor_effect.selector();
        let disposition = policy.rules.get(&selector).ok_or_else(|| {
            format!(
                "unclassified vendor effect at index {vendor_index}: {}",
                selector.canonical()
            )
        })?;
        used_rules.insert(selector.clone());
        match disposition {
            EffectDisposition::Required | EffectDisposition::PlatformOwned => {
                let Some(rust_effect) = rust.get(rust_index) else {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "required {} is missing from Rust effects",
                        selector.canonical()
                    )));
                };
                if !vendor_effect.equivalent(rust_effect) {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "vendor effect {} does not match Rust effect {} at index {rust_index}",
                        selector.canonical(),
                        rust_effect.selector().canonical()
                    )));
                }
                rust_index += 1;
            }
            EffectDisposition::ReplacedByAsync { condition, timeout } => {
                let Some(ContractEffect::AwaitReady {
                    condition: rust_condition,
                    timeout: rust_timeout,
                }) = rust.get(rust_index)
                else {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "{} requires one Rust await-ready replacement",
                        selector.canonical()
                    )));
                };
                if rust_condition.id() != *condition || rust_timeout != timeout {
                    return Ok(EffectComparisonVerdict::Mismatch(format!(
                        "{} requires await-ready {condition} {}, received await-ready {} {}",
                        selector.canonical(),
                        timeout.canonical(),
                        rust_condition.id(),
                        rust_timeout.canonical(),
                    )));
                }
                rust_index += 1;
            }
            EffectDisposition::PlatformProvidedInput { input } => {
                let expected = ContractEffect::PlatformProvidedInput {
                    input: input.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::PlatformProvidedService { service } => {
                let expected = ContractEffect::PlatformProvidedService {
                    service: service.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::PublishedEvent { event } => {
                let expected = ContractEffect::PublishedEvent {
                    event: event.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::InitializationPrerequisite { prerequisite } => {
                let expected = ContractEffect::InitializationPrerequisite {
                    prerequisite: prerequisite.clone(),
                };
                rust_index = match consume_replacement(&selector, &expected, rust, rust_index) {
                    Ok(next) => next,
                    Err(reason) => return Ok(EffectComparisonVerdict::Mismatch(reason)),
                };
            }
            EffectDisposition::AllowedOmission(_) => {
                if rust
                    .get(rust_index)
                    .is_some_and(|rust_effect| vendor_effect.equivalent(rust_effect))
                {
                    rust_index += 1;
                }
            }
            EffectDisposition::Forbidden => {
                return Err(format!(
                    "forbidden vendor effect at index {vendor_index}: {}",
                    selector.canonical()
                )
                .into());
            }
        }
    }
    if let Some(extra) = rust.get(rust_index) {
        return Err(format!(
            "unclassified extra Rust effect at index {rust_index}: {}",
            extra.selector().canonical()
        )
        .into());
    }
    for (selector, disposition) in &policy.rules {
        if disposition != &EffectDisposition::Forbidden && !used_rules.contains(selector) {
            return Err(format!(
                "declared effect rule was not exercised: {}",
                selector.canonical()
            )
            .into());
        }
    }
    Ok(EffectComparisonVerdict::Match)
}

fn consume_replacement(
    vendor_selector: &EffectSelector,
    expected: &ContractEffect,
    rust: &[ContractEffect],
    rust_index: usize,
) -> core::result::Result<usize, String> {
    let Some(actual) = rust.get(rust_index) else {
        return Err(format!(
            "{} requires Rust replacement {}, but it is missing",
            vendor_selector.canonical(),
            expected.selector().canonical(),
        ));
    };
    if !expected.equivalent(actual) {
        return Err(format!(
            "{} requires Rust replacement {}, received {} at index {rust_index}",
            vendor_selector.canonical(),
            expected.selector().canonical(),
            actual.selector().canonical(),
        ));
    }
    Ok(rust_index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register() -> RegisterId {
        RegisterId {
            address: 0x2010_7030,
            width: 32,
            name: "PHY_AGC_ORACLE.AGC_ANTENNA_CONTROL".to_owned(),
        }
    }

    fn read() -> ContractEffect {
        ContractEffect::MmioRead {
            register: register(),
            value: ContractValue::ReadResult { ordinal: 0 },
        }
    }

    #[test]
    fn exact_policy_rejects_an_unclassified_vendor_effect() {
        let policy = EffectPolicy::new(
            EffectComparison::ExactEffectsV1,
            [(EffectSelector::Delay, EffectDisposition::Required)],
        )
        .unwrap();
        let error = compare_effects(&[read()], &[read()], &policy).unwrap_err();
        assert!(error.to_string().contains("unclassified vendor effect"));
    }

    #[test]
    fn exact_policy_rejects_an_extra_rust_effect() {
        let selector = read().selector();
        let policy = EffectPolicy::new(
            EffectComparison::ExactEffectsV1,
            [(selector, EffectDisposition::Required)],
        )
        .unwrap();
        let error = compare_effects(&[read()], &[read(), read()], &policy).unwrap_err();
        assert!(error.to_string().contains("unclassified extra Rust effect"));
    }

    #[test]
    fn blocking_effect_requires_an_explicit_await_ready_replacement() {
        let policy = EffectPolicy::new(
            EffectComparison::ExactEffectsV1,
            [(
                EffectSelector::Delay,
                EffectDisposition::ReplacedByAsync {
                    condition: "iq-estimator-ready".to_owned(),
                    timeout: Timeout::Attempts(100),
                },
            )],
        )
        .unwrap();
        let vendor = [ContractEffect::Delay {
            micros: ContractValue::Concrete(1),
        }];
        assert!(matches!(
            compare_effects(&vendor, &[], &policy).unwrap(),
            EffectComparisonVerdict::Mismatch(_)
        ));
        let rust = [ContractEffect::AwaitReady {
            condition: ReadyCondition::Named("iq-estimator-ready".to_owned()),
            timeout: Timeout::Attempts(100),
        }];
        assert_eq!(
            compare_effects(&vendor, &rust, &policy).unwrap(),
            EffectComparisonVerdict::Match
        );
    }

    #[test]
    fn omission_reason_and_platform_operation_vocabularies_are_closed() {
        assert!(OmissionReason::parse("debug-diagnostic", 1).is_ok());
        assert!(OmissionReason::parse("whatever", 1).is_err());
        assert!(PlatformOperation::parse("nvs-calibration-cache", 1).is_ok());
        assert!(PlatformOperation::parse("vendor-magic", 1).is_err());
    }

    #[test]
    fn effect_rule_parser_is_closed_and_restricts_omissions() {
        assert_eq!(
            parse_effect_rule(
                "platform-call debug-diagnostic allowed-omission debug-diagnostic",
                7,
            )
            .unwrap(),
            (
                EffectSelector::PlatformCall {
                    operation: PlatformOperation::DebugDiagnostic,
                },
                EffectDisposition::AllowedOmission(OmissionReason::DebugDiagnostic),
            )
        );
        assert!(parse_effect_rule("vendor-effect magic required", 8).is_err());
        assert!(
            parse_effect_rule(
                "mmio-write 32 0x20107030 allowed-omission debug-diagnostic",
                9,
            )
            .is_err()
        );
    }

    #[test]
    fn semantic_boundary_dispositions_require_exact_typed_replacements() {
        let state_field = |field: &str| StateField {
            projection: "sta".to_owned(),
            field: field.to_owned(),
            width: 32,
        };
        let vendor = [
            read(),
            ContractEffect::PlatformCall {
                operation: PlatformOperation::RtosSchedulingAdapter,
                arguments: Vec::new(),
            },
            ContractEffect::StateWrite {
                field: state_field("event"),
                value: ContractValue::Concrete(1),
            },
            ContractEffect::StateWrite {
                field: state_field("initialized"),
                value: ContractValue::Concrete(1),
            },
        ];
        let policy = EffectPolicy::new(
            EffectComparison::ExactEffectsV1,
            [
                (
                    vendor[0].selector(),
                    EffectDisposition::PlatformProvidedInput {
                        input: "station-mac".to_owned(),
                    },
                ),
                (
                    vendor[1].selector(),
                    EffectDisposition::PlatformProvidedService {
                        service: "embassy-wakeup".to_owned(),
                    },
                ),
                (
                    vendor[2].selector(),
                    EffectDisposition::PublishedEvent {
                        event: "rx-ready".to_owned(),
                    },
                ),
                (
                    vendor[3].selector(),
                    EffectDisposition::InitializationPrerequisite {
                        prerequisite: "mac-clock-enabled".to_owned(),
                    },
                ),
            ],
        )
        .unwrap();
        let rust = [
            ContractEffect::PlatformProvidedInput {
                input: "station-mac".to_owned(),
            },
            ContractEffect::PlatformProvidedService {
                service: "embassy-wakeup".to_owned(),
            },
            ContractEffect::PublishedEvent {
                event: "rx-ready".to_owned(),
            },
            ContractEffect::InitializationPrerequisite {
                prerequisite: "mac-clock-enabled".to_owned(),
            },
        ];
        assert_eq!(
            compare_effects(&vendor, &rust, &policy).unwrap(),
            EffectComparisonVerdict::Match
        );
        assert!(matches!(
            compare_effects(&vendor, &rust[..3], &policy).unwrap(),
            EffectComparisonVerdict::Mismatch(reason)
                if reason.contains("initialization-prerequisite")
        ));
    }

    #[test]
    fn boundary_effect_selectors_are_valid_contract_rules() {
        for rule in [
            "platform-provided-input station-mac required",
            "platform-provided-service embassy-wakeup required",
            "published-event rx-success required",
            "initialization-prerequisite power-irqs-disabled required",
        ] {
            assert!(parse_effect_rule(rule, 17).is_ok(), "{rule}");
        }
    }

    #[test]
    fn semantic_boundary_rule_syntax_is_closed_and_canonical() {
        for rule in [
            "mmio-read 32 0x20107030 platform-provided-input station-mac",
            "platform-call rtos-scheduling-adapter platform-provided-service embassy-wakeup",
            "state-write 32 sta.event published-event rx-ready",
            "mmio-write 32 0x20107030 initialization-prerequisite mac-clock-enabled",
        ] {
            let (selector, disposition) = parse_effect_rule(rule, 11).unwrap();
            assert_eq!(
                format!("{} {}", selector.canonical(), disposition.canonical()),
                rule
            );
        }
        assert!(
            parse_effect_rule(
                "mmio-write 32 0x20107030 platform-provided-input station-mac",
                12,
            )
            .is_err()
        );
        assert!(
            parse_effect_rule("platform-call random published-event Invalid/Event", 13,).is_err()
        );
    }
}
