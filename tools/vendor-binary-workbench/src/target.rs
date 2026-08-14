//! Architecture and calling-convention selection at the workbench boundary.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub(crate) enum TargetError {
    #[error("cannot read target specification {}", path.display())]
    #[diagnostic(code(workbench::target::read))]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{message}")]
    #[diagnostic(code(workbench::target::invalid))]
    Invalid {
        message: String,
        #[source_code]
        src: NamedSource<String>,
        #[label("invalid target specification")]
        span: SourceSpan,
    },
    #[error("target {id} has an architecture/calling-convention mismatch")]
    #[diagnostic(
        code(workbench::target::abi_mismatch),
        help("select a calling convention defined for the target architecture")
    )]
    AbiMismatch { id: String },
    #[error("target {id} is valid but its architecture backend is not implemented")]
    #[diagnostic(code(workbench::target::backend_unavailable))]
    BackendUnavailable { id: String },
    #[error("target {id} has no knowledge provider")]
    #[diagnostic(
        code(workbench::target::missing_harness),
        help("attach a compatible chip pack to the project")
    )]
    MissingKnowledgeProvider { id: String },
    #[error("target {id} selects unavailable knowledge provider {provider:?}")]
    #[diagnostic(code(workbench::target::harness_unavailable))]
    KnowledgeProviderUnavailable { id: String, provider: String },
}

type Result<T> = std::result::Result<T, TargetError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Architecture {
    Riscv32,
    Xtensa,
    ArmThumb,
}

impl Architecture {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Riscv32 => "riscv32",
            Self::Xtensa => "xtensa",
            Self::ArmThumb => "arm-thumb",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CallingConvention {
    RiscvIlp32,
    XtensaCall0,
    XtensaWindowed,
    #[serde(rename = "aapcs32-softfloat")]
    Aapcs32SoftFloat,
    #[serde(rename = "aapcs32-hardfloat")]
    Aapcs32HardFloat,
}

impl CallingConvention {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::RiscvIlp32 => "riscv-ilp32",
            Self::XtensaCall0 => "xtensa-call0",
            Self::XtensaWindowed => "xtensa-windowed",
            Self::Aapcs32SoftFloat => "aapcs32-softfloat",
            Self::Aapcs32HardFloat => "aapcs32-hardfloat",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Endianness {
    Little,
    Big,
}

impl Endianness {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Little => "little",
            Self::Big => "big",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetSpec {
    pub(crate) id: String,
    pub(crate) knowledge_provider: Option<String>,
    pub(crate) architecture: Architecture,
    pub(crate) calling_convention: CallingConvention,
    pub(crate) endianness: Endianness,
    pub(crate) pointer_width: u8,
    pub(crate) rust_target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct TargetDocument {
    schema: u32,
    id: String,
    architecture: Architecture,
    calling_convention: CallingConvention,
    endianness: Endianness,
    pointer_width: u8,
    rust_target: String,
}

impl TargetSpec {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|source| TargetError::Read {
            path: path.to_owned(),
            source,
        })?;
        let document: TargetDocument = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span().unwrap_or(0..input.len().min(1));
            TargetError::Invalid {
                message: format!("invalid target TOML: {error}"),
                src: NamedSource::new(path.display().to_string(), input.clone()),
                span: (span.start, span.len().max(1)).into(),
            }
        })?;
        if document.schema != 3 {
            return Err(invalid(path, &input, "target TOML requires schema = 3"));
        }
        let target = Self {
            id: document.id,
            knowledge_provider: None,
            architecture: document.architecture,
            calling_convention: document.calling_convention,
            endianness: document.endianness,
            pointer_width: document.pointer_width,
            rust_target: document.rust_target,
        };
        target.validate_pair()?;
        Ok(target)
    }

    fn validate_pair(&self) -> Result<()> {
        let valid = matches!(
            (self.architecture, self.calling_convention),
            (Architecture::Riscv32, CallingConvention::RiscvIlp32)
                | (Architecture::Xtensa, CallingConvention::XtensaCall0)
                | (Architecture::Xtensa, CallingConvention::XtensaWindowed)
                | (
                    Architecture::ArmThumb,
                    CallingConvention::Aapcs32SoftFloat | CallingConvention::Aapcs32HardFloat
                )
        );
        if !valid {
            return Err(TargetError::AbiMismatch {
                id: self.id.clone(),
            });
        }
        Ok(())
    }

    /// The transition facade still contains only the RV32 backend. Recognized
    /// future targets fail here instead of reaching an incompatible decoder.
    pub(crate) fn require_available_backend(&self) -> Result<()> {
        if self.architecture != Architecture::Riscv32
            || self.calling_convention != CallingConvention::RiscvIlp32
            || self.endianness != Endianness::Little
            || self.pointer_width != 32
        {
            return Err(TargetError::BackendUnavailable {
                id: self.id.clone(),
            });
        }
        Ok(())
    }

    /// Executable chip knowledge is selected by a reviewed chip pack.
    pub(crate) fn require_available_knowledge_provider(&self) -> Result<&str> {
        let provider = self.knowledge_provider.as_deref().ok_or_else(|| {
            TargetError::MissingKnowledgeProvider {
                id: self.id.clone(),
            }
        })?;
        if !crate::harnesses::is_available(provider) {
            return Err(TargetError::KnowledgeProviderUnavailable {
                id: self.id.clone(),
                provider: provider.to_owned(),
            });
        }
        Ok(provider)
    }
}

fn invalid(path: &Path, input: &str, message: impl Into<String>) -> TargetError {
    TargetError::Invalid {
        message: message.into(),
        src: NamedSource::new(path.display().to_string(), input.to_owned()),
        span: (0, 1).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_spec(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "open-radio-workbench-target-{}-{name}.toml",
            std::process::id(),
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_an_explicit_riscv_abi_pair() {
        let path = write_spec(
            "riscv",
            "schema = 3\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imafc-unknown-none-elf\"\n",
        );
        let target = TargetSpec::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        target.require_available_backend().unwrap();
    }

    #[test]
    fn rejects_chip_resources_in_the_architecture_target() {
        let path = write_spec(
            "memory-map",
            "schema = 3\nid = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\nmemory-map = \"maps/device.toml\"\n",
        );
        let error = TargetSpec::load(&path).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        assert!(error.to_string().contains("unknown field `memory-map`"));
    }

    #[test]
    fn generic_target_has_no_platform_harness() {
        let path = write_spec(
            "generic-riscv",
            "schema = 3\nid = \"generic-riscv\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\n",
        );
        let target = TargetSpec::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        target.require_available_backend().unwrap();
        assert!(
            target
                .require_available_knowledge_provider()
                .unwrap_err()
                .to_string()
                .contains("no knowledge provider")
        );
    }

    #[test]
    fn rejects_harness_selection_in_a_target_spec() {
        let path = write_spec(
            "target-harness",
            "schema = 3\nid = \"fixture\"\nharness = \"fixture\"\narchitecture = \"riscv32\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"riscv32imac-unknown-none-elf\"\n",
        );
        let error = TargetSpec::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(error.to_string().contains("unknown field `harness`"));
    }

    #[test]
    fn rejects_an_architecture_abi_mismatch() {
        let path = write_spec(
            "mismatch",
            "schema = 3\nid = \"fixture\"\narchitecture = \"arm-thumb\"\ncalling-convention = \"riscv-ilp32\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"thumbv7em-none-eabi\"\n",
        );
        let error = TargetSpec::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(error.to_string().contains("mismatch"));
    }

    #[test]
    fn recognizes_arm_float_abi_without_claiming_a_backend() {
        let path = write_spec(
            "arm-hardfloat",
            "schema = 3\nid = \"fixture\"\narchitecture = \"arm-thumb\"\ncalling-convention = \"aapcs32-hardfloat\"\nendianness = \"little\"\npointer-width = 32\nrust-target = \"thumbv7em-none-eabihf\"\n",
        );
        let target = TargetSpec::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(
            target
                .require_available_backend()
                .unwrap_err()
                .to_string()
                .contains("not implemented")
        );
    }
}
