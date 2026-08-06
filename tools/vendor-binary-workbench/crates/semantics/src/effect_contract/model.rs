//! Effect Contract data model and canonical representation.

use super::*;

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
    pub(super) fn parse(value: &str, line: usize) -> Result<Self> {
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
