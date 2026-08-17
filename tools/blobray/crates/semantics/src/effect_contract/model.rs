//! Effect Contract data model and canonical representation.

use super::*;
use serde::Deserialize;

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
    reason = "Effect Contract v2 defines concrete, state, and async values before all pilots consume them"
)]
pub enum ContractValue {
    Concrete(u32),
    ReadResult { ordinal: u32 },
    Symbolic(SymbolicValue),
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
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
    reason = "named and MMIO readiness are both part of the v2 async contract"
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
#[allow(
    dead_code,
    reason = "attempt and deadline timeouts are both part of the v2 async contract"
)]
pub enum Timeout {
    Attempts(u32),
    DeadlineMicros(u32),
}

impl Timeout {
    pub(super) fn canonical(self) -> String {
        match self {
            Self::Attempts(count) => format!("attempts={count}"),
            Self::DeadlineMicros(micros) => format!("deadline-us={micros}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the v2 schema is complete before state and async pilots are connected"
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
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EffectSelector {
    MmioRead {
        width: u8,
        address: u32,
    },
    MmioWrite {
        width: u8,
        address: u32,
    },
    StateRead {
        width: u8,
        field: String,
    },
    StateWrite {
        width: u8,
        field: String,
    },
    Delay,
    AwaitReady {
        condition: String,
    },
    PlatformCall {
        operation: PlatformOperation,
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
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
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
            Self::Fence {
                fm,
                predecessor,
                successor,
            } => format!("fence {fm:#04x} {predecessor:#04x} {successor:#04x}"),
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
            Self::Fence {
                fm,
                predecessor,
                successor,
            } => EffectSelector::Fence {
                fm: *fm,
                predecessor: *predecessor,
                successor: *successor,
            },
        }
    }

    pub(super) fn equivalent(&self, other: &Self) -> bool {
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
            (
                Self::Fence {
                    fm: left_fm,
                    predecessor: left_predecessor,
                    successor: left_successor,
                },
                Self::Fence {
                    fm: right_fm,
                    predecessor: right_predecessor,
                    successor: right_successor,
                },
            ) => {
                left_fm == right_fm
                    && left_predecessor == right_predecessor
                    && left_successor == right_successor
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum OmissionReason {
    DebugDiagnostic,
    NvsCalibrationCache,
    RtosSchedulingAdapter,
    UnusedInstrumentation,
}

impl OmissionReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::DebugDiagnostic => "debug-diagnostic",
            Self::NvsCalibrationCache => "nvs-calibration-cache",
            Self::RtosSchedulingAdapter => "rtos-scheduling-adapter",
            Self::UnusedInstrumentation => "unused-instrumentation",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum EffectDisposition {
    Required,
    ReplacedByAsync { condition: String, timeout: Timeout },
    PlatformProvidedInput { input: String },
    PlatformProvidedService { service: String },
    PublishedEvent { event: String },
    InitializationPrerequisite { prerequisite: String },
    RustAddition(RustAdditionReason),
    AllowedOmission(OmissionReason),
    PlatformOwned,
    Forbidden,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum RustAdditionReason {
    DeviceOrdering,
}

impl RustAdditionReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::DeviceOrdering => "device-ordering",
        }
    }
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
            Self::RustAddition(reason) => format!("rust-addition {}", reason.label()),
            Self::AllowedOmission(reason) => {
                format!("allowed-omission {}", reason.label())
            }
            Self::PlatformOwned => "platform-owned".to_owned(),
            Self::Forbidden => "forbidden".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum EffectComparison {
    ExactEffectsV2,
}

impl EffectComparison {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ExactEffectsV2 => "exact-effects-v2",
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
            validate_effect_rule(&selector, &disposition)?;
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

fn validate_effect_rule(selector: &EffectSelector, disposition: &EffectDisposition) -> Result<()> {
    let width = match selector {
        EffectSelector::MmioRead { width, .. }
        | EffectSelector::MmioWrite { width, .. }
        | EffectSelector::StateRead { width, .. }
        | EffectSelector::StateWrite { width, .. } => Some(*width),
        _ => None,
    };
    if width.is_some_and(|width| !matches!(width, 8 | 16 | 32)) {
        return Err("effect width must be 8, 16, or 32".into());
    }
    for id in match selector {
        EffectSelector::PlatformProvidedInput { input } => [Some(input.as_str()), None],
        EffectSelector::PlatformProvidedService { service } => [Some(service.as_str()), None],
        EffectSelector::PublishedEvent { event } => [Some(event.as_str()), None],
        EffectSelector::InitializationPrerequisite { prerequisite } => {
            [Some(prerequisite.as_str()), None]
        }
        _ => [None, None],
    }
    .into_iter()
    .flatten()
    {
        validate_boundary_id(id)?;
    }
    match disposition {
        EffectDisposition::ReplacedByAsync { condition, timeout } => {
            validate_boundary_id(condition)?;
            let count = match timeout {
                Timeout::Attempts(count) | Timeout::DeadlineMicros(count) => *count,
            };
            if count == 0 {
                return Err("async replacement timeout must be non-zero".into());
            }
        }
        EffectDisposition::PlatformProvidedInput { input } => validate_boundary_id(input)?,
        EffectDisposition::PlatformProvidedService { service } => validate_boundary_id(service)?,
        EffectDisposition::PublishedEvent { event } => validate_boundary_id(event)?,
        EffectDisposition::InitializationPrerequisite { prerequisite } => {
            validate_boundary_id(prerequisite)?
        }
        _ => {}
    }
    match (selector, disposition) {
        (EffectSelector::PlatformCall { .. }, EffectDisposition::AllowedOmission(_))
        | (EffectSelector::PlatformCall { .. }, EffectDisposition::PlatformOwned)
        | (_, EffectDisposition::Required | EffectDisposition::Forbidden)
        | (
            EffectSelector::Delay | EffectSelector::MmioRead { .. },
            EffectDisposition::ReplacedByAsync { .. },
        )
        | (
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
        | (EffectSelector::Fence { .. }, EffectDisposition::RustAddition(_))
        | (_, EffectDisposition::InitializationPrerequisite { .. }) => Ok(()),
        (_, EffectDisposition::RustAddition(_)) => {
            Err("rust-addition applies only to fence effects".into())
        }
        (_, EffectDisposition::AllowedOmission(_)) => {
            Err("allowed-omission applies only to platform-call effects".into())
        }
        (_, EffectDisposition::PlatformOwned) => {
            Err("platform-owned applies only to platform-call effects".into())
        }
        (_, EffectDisposition::ReplacedByAsync { .. }) => {
            Err("replaced-by-async applies only to delay or MMIO-read effects".into())
        }
        (_, EffectDisposition::PlatformProvidedInput { .. }) => {
            Err("platform-provided-input applies only to read or platform-call effects".into())
        }
        (_, EffectDisposition::PlatformProvidedService { .. }) => {
            Err("platform-provided-service applies only to platform-call effects".into())
        }
        (_, EffectDisposition::PublishedEvent { .. }) => {
            Err("published-event applies only to state-write or platform-call effects".into())
        }
    }
}

fn validate_boundary_id(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!("invalid effect boundary id {value:?}").into());
    }
    Ok(())
}
