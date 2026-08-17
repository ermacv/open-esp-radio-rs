//! Typed data model shared by project-doctor collectors and renderers.

use std::{fmt, path::PathBuf};

use serde::{Serialize, Serializer, ser::SerializeMap as _};

use super::super::{
    project_function_doctor::FunctionDoctorReport, project_ir_doctor::IrDoctorReport,
};

#[derive(Serialize)]
pub(super) struct DoctorReport {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: DoctorStatus,
    pub(super) project: IdentityReport,
    pub(super) target: IdentityReport,
    pub(super) capabilities: Vec<CapabilityReport>,
    pub(super) ir_build: IrDoctorReport,
    pub(super) function_workspace: FunctionDoctorReport,
    pub(super) run_spec: RunSpecReport,
    pub(super) inputs: Vec<InputReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) diagnostics: Vec<DoctorDiagnostic>,
    pub(super) errors: usize,
    pub(super) warnings: usize,
    pub(super) valid_inputs: usize,
}

impl DoctorReport {
    pub(super) fn new(
        project_id: &str,
        project_path: PathBuf,
        target_id: &str,
        target_path: PathBuf,
        ir_build: IrDoctorReport,
        function_workspace: FunctionDoctorReport,
        run_spec: RunSpecReport,
    ) -> Self {
        Self {
            schema: 2,
            command: "project doctor",
            status: DoctorStatus::Valid,
            project: IdentityReport {
                id: project_id.to_owned(),
                path: project_path,
            },
            target: IdentityReport {
                id: target_id.to_owned(),
                path: target_path,
            },
            capabilities: Vec::new(),
            ir_build,
            function_workspace,
            run_spec,
            inputs: Vec::new(),
            diagnostics: Vec::new(),
            errors: 0,
            warnings: 0,
            valid_inputs: 0,
        }
    }

    pub(super) fn absorb(&mut self, errors: usize, warnings: usize) {
        self.errors += errors;
        self.warnings += warnings;
        self.status = DoctorStatus::from_counts(self.errors, self.warnings);
    }

    pub(super) fn error(&mut self) {
        self.absorb(1, 0);
    }

    pub(super) fn warning(&mut self, message: impl Into<String>) {
        self.diagnostics.push(DoctorDiagnostic {
            level: "warning",
            message: message.into(),
        });
        self.absorb(0, 1);
    }

    pub(super) fn capability(&mut self, capability: CapabilityReport) {
        self.capabilities.push(capability);
    }

    pub(super) const fn succeeded(&self) -> bool {
        self.errors == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DoctorStatus {
    Valid,
    ValidWithWarnings,
    Invalid,
}

impl DoctorStatus {
    const fn from_counts(errors: usize, warnings: usize) -> Self {
        if errors != 0 {
            Self::Invalid
        } else if warnings != 0 {
            Self::ValidWithWarnings
        } else {
            Self::Valid
        }
    }
}

impl fmt::Display for DoctorStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Valid => "valid",
            Self::ValidWithWarnings => "valid-with-warnings",
            Self::Invalid => "invalid",
        })
    }
}

#[derive(Serialize)]
pub(super) struct IdentityReport {
    pub(super) id: String,
    pub(super) path: PathBuf,
}

#[derive(Serialize)]
pub(super) struct CapabilityReport {
    pub(super) name: &'static str,
    pub(super) status: &'static str,
    #[serde(serialize_with = "serialize_fields")]
    pub(super) details: Vec<ReportField>,
}

impl CapabilityReport {
    pub(super) fn new(name: &'static str, status: &'static str) -> Self {
        Self {
            name,
            status,
            details: Vec::new(),
        }
    }

    pub(super) fn field(mut self, name: &'static str, value: impl Into<ReportValue>) -> Self {
        self.details.push(ReportField {
            name,
            value: value.into(),
        });
        self
    }
}

pub(super) struct ReportField {
    pub(super) name: &'static str,
    pub(super) value: ReportValue,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum ReportValue {
    Unsigned(u64),
    Boolean(bool),
    String(String),
    Strings(Vec<String>),
}

impl fmt::Display for ReportValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsigned(value) => value.fmt(formatter),
            Self::Boolean(value) => value.fmt(formatter),
            Self::String(value) => value.fmt(formatter),
            Self::Strings(values) if values.is_empty() => formatter.write_str("-"),
            Self::Strings(values) => formatter.write_str(&values.join(",")),
        }
    }
}

impl From<usize> for ReportValue {
    fn from(value: usize) -> Self {
        Self::Unsigned(value as u64)
    }
}

impl From<u64> for ReportValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for ReportValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<String> for ReportValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for ReportValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Vec<String>> for ReportValue {
    fn from(value: Vec<String>) -> Self {
        Self::Strings(value)
    }
}

fn serialize_fields<S>(fields: &[ReportField], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = serializer.serialize_map(Some(fields.len()))?;
    for field in fields {
        map.serialize_entry(field.name, &field.value)?;
    }
    map.end()
}

#[derive(Serialize)]
pub(super) struct DoctorDiagnostic {
    pub(super) level: &'static str,
    pub(super) message: String,
}

#[derive(Serialize)]
pub(super) struct RunSpecReport {
    pub(super) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) diagnostic: Option<&'static str>,
}

impl RunSpecReport {
    pub(super) fn configured(path: PathBuf) -> Self {
        Self {
            status: "available",
            path: Some(path),
            diagnostic: None,
        }
    }

    pub(super) const fn missing() -> Self {
        Self {
            status: "not-configured",
            path: None,
            diagnostic: Some("artifact-bindings-unavailable"),
        }
    }
}

#[derive(Serialize)]
pub(super) struct InputReport {
    pub(super) role: String,
    pub(super) status: &'static str,
    pub(super) path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) container: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) objects: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) skipped_members: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) symbol_facts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) code_definitions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) exported_definitions: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) undefined: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}
