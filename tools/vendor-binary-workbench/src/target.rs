//! Architecture and calling-convention selection at the workbench boundary.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use miette::{Diagnostic, NamedSource, SourceSpan};
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
    #[error("target {id} has no platform harness")]
    #[diagnostic(
        code(workbench::target::missing_harness),
        help("attach a compatible platform pack to the project")
    )]
    MissingHarness { id: String },
    #[error("target {id} selects unavailable harness {harness:?}")]
    #[diagnostic(code(workbench::target::harness_unavailable))]
    HarnessUnavailable { id: String, harness: String },
}

type Result<T> = std::result::Result<T, TargetError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Architecture {
    Riscv32,
    Xtensa,
    ArmThumb,
}

impl Architecture {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "riscv32" => Some(Self::Riscv32),
            "xtensa" => Some(Self::Xtensa),
            "arm-thumb" => Some(Self::ArmThumb),
            _ => None,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Riscv32 => "riscv32",
            Self::Xtensa => "xtensa",
            Self::ArmThumb => "arm-thumb",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallingConvention {
    RiscvIlp32,
    XtensaCall0,
    XtensaWindowed,
    Aapcs32SoftFloat,
    Aapcs32HardFloat,
}

impl CallingConvention {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "riscv-ilp32" => Some(Self::RiscvIlp32),
            "xtensa-call0" => Some(Self::XtensaCall0),
            "xtensa-windowed" => Some(Self::XtensaWindowed),
            "aapcs32-softfloat" => Some(Self::Aapcs32SoftFloat),
            "aapcs32-hardfloat" => Some(Self::Aapcs32HardFloat),
            _ => None,
        }
    }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Endianness {
    Little,
    Big,
}

impl Endianness {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "little" => Some(Self::Little),
            "big" => Some(Self::Big),
            _ => None,
        }
    }

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
    pub(crate) harness: Option<String>,
    pub(crate) architecture: Architecture,
    pub(crate) calling_convention: CallingConvention,
    pub(crate) endianness: Endianness,
    pub(crate) pointer_width: u8,
    pub(crate) rust_target: String,
    pub(crate) memory_map: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) pac_bindings: Option<PathBuf>,
    pub(crate) profiles: Option<PathBuf>,
    pub(crate) dispositions: Option<PathBuf>,
    pub(crate) evidence_baseline: Option<PathBuf>,
}

impl TargetSpec {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path).map_err(|source| TargetError::Read {
            path: path.to_owned(),
            source,
        })?;
        let mut schema = None;
        let mut id = None;
        let mut architecture = None;
        let mut calling_convention = None;
        let mut endianness = None;
        let mut pointer_width = None;
        let mut rust_target = None;
        let mut memory_map = None;
        let mut svd_paths = Vec::new();
        let mut pac_bindings = None;
        let mut profiles = None;
        let mut dispositions = None;
        let mut evidence_baseline = None;
        let base = path.parent().unwrap_or_else(|| Path::new("."));

        for (index, raw_line) in input.lines().enumerate() {
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (directive, value) = line
                .split_once(char::is_whitespace)
                .map(|(directive, value)| (directive, value.trim()))
                .filter(|(_, value)| !value.is_empty())
                .ok_or_else(|| invalid(path, &input, index, "target directive requires a value"))?;
            match directive {
                "schema" => set_once(
                    &mut schema,
                    value
                        .parse::<u32>()
                        .map_err(|_| invalid(path, &input, index, "schema must be an integer"))?,
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "target" => {
                    if value.chars().any(char::is_whitespace) {
                        return Err(invalid(path, &input, index, "target id must be one token"));
                    }
                    set_once(&mut id, value.to_owned(), directive, path, &input, index)?;
                }
                "architecture" => set_once(
                    &mut architecture,
                    Architecture::parse(value).ok_or_else(|| {
                        invalid(
                            path,
                            &input,
                            index,
                            format!("unsupported architecture {value:?}"),
                        )
                    })?,
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "calling-convention" => set_once(
                    &mut calling_convention,
                    CallingConvention::parse(value).ok_or_else(|| {
                        invalid(
                            path,
                            &input,
                            index,
                            format!("unsupported calling convention {value:?}"),
                        )
                    })?,
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "endianness" => set_once(
                    &mut endianness,
                    Endianness::parse(value).ok_or_else(|| {
                        invalid(
                            path,
                            &input,
                            index,
                            format!("unsupported endianness {value:?}"),
                        )
                    })?,
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "pointer-width" => set_once(
                    &mut pointer_width,
                    value.parse::<u8>().map_err(|_| {
                        invalid(path, &input, index, "pointer width must be an integer")
                    })?,
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "rust-target" => {
                    set_token(&mut rust_target, value, directive, path, &input, index)?;
                }
                "memory-map" => set_once(
                    &mut memory_map,
                    resolve_path(base, value),
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "svd" => svd_paths.push(resolve_path(base, value)),
                "pac-bindings" => set_once(
                    &mut pac_bindings,
                    resolve_path(base, value),
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "profiles" => set_once(
                    &mut profiles,
                    resolve_path(base, value),
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "dispositions" => set_once(
                    &mut dispositions,
                    resolve_path(base, value),
                    directive,
                    path,
                    &input,
                    index,
                )?,
                "evidence-baseline" => set_once(
                    &mut evidence_baseline,
                    resolve_path(base, value),
                    directive,
                    path,
                    &input,
                    index,
                )?,
                _ => {
                    return Err(invalid(
                        path,
                        &input,
                        index,
                        format!("unknown target directive {directive:?}"),
                    ));
                }
            }
        }

        if schema != Some(1) {
            return Err(invalid(path, &input, 0, "target spec requires schema 1"));
        }
        let target = Self {
            id: id.ok_or_else(|| invalid(path, &input, 0, "target spec has no target id"))?,
            harness: None,
            architecture: architecture
                .ok_or_else(|| invalid(path, &input, 0, "target spec has no architecture"))?,
            calling_convention: calling_convention
                .ok_or_else(|| invalid(path, &input, 0, "target spec has no calling-convention"))?,
            endianness: endianness
                .ok_or_else(|| invalid(path, &input, 0, "target spec has no endianness"))?,
            pointer_width: pointer_width
                .ok_or_else(|| invalid(path, &input, 0, "target spec has no pointer-width"))?,
            rust_target: rust_target
                .ok_or_else(|| invalid(path, &input, 0, "target spec has no rust-target"))?,
            memory_map,
            svd_paths,
            pac_bindings,
            profiles,
            dispositions,
            evidence_baseline,
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

    /// Harness selection is supplied by a reviewed platform pack.
    pub(crate) fn require_available_harness(&self) -> Result<&str> {
        let harness = self
            .harness
            .as_deref()
            .ok_or_else(|| TargetError::MissingHarness {
                id: self.id.clone(),
            })?;
        if !crate::harnesses::is_available(harness) {
            return Err(TargetError::HarnessUnavailable {
                id: self.id.clone(),
                harness: harness.to_owned(),
            });
        }
        Ok(harness)
    }
}

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    directive: &str,
    path: &Path,
    input: &str,
    line_index: usize,
) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(invalid(
            path,
            input,
            line_index,
            format!("duplicate {directive} directive"),
        ));
    }
    Ok(())
}

fn set_token(
    slot: &mut Option<String>,
    value: &str,
    directive: &str,
    path: &Path,
    input: &str,
    line_index: usize,
) -> Result<()> {
    if value.chars().any(char::is_whitespace) {
        return Err(invalid(
            path,
            input,
            line_index,
            format!("{directive} must be one token"),
        ));
    }
    set_once(slot, value.to_owned(), directive, path, input, line_index)
}

fn invalid(path: &Path, input: &str, line_index: usize, message: impl Into<String>) -> TargetError {
    let offset = input
        .split_inclusive('\n')
        .take(line_index)
        .map(str::len)
        .sum::<usize>();
    let length = input
        .split_inclusive('\n')
        .nth(line_index)
        .map(|line| line.trim_end_matches(['\r', '\n']).len())
        .unwrap_or(0)
        .max(1);
    TargetError::Invalid {
        message: message.into(),
        src: NamedSource::new(path.display().to_string(), input.to_owned()),
        span: (offset, length).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_spec(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "open-radio-workbench-target-{}-{name}.spec",
            std::process::id(),
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_an_explicit_riscv_abi_pair() {
        let path = write_spec(
            "riscv",
            "schema 1\ntarget fixture\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imafc-unknown-none-elf\n",
        );
        let target = TargetSpec::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        target.require_available_backend().unwrap();
    }

    #[test]
    fn resolves_an_optional_target_memory_map() {
        let path = write_spec(
            "memory-map",
            "schema 1\ntarget fixture\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imac-unknown-none-elf\nmemory-map maps/device.toml\n",
        );
        let target = TargetSpec::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            target.memory_map,
            Some(path.parent().unwrap().join("maps/device.toml"))
        );
    }

    #[test]
    fn generic_target_has_no_platform_harness() {
        let path = write_spec(
            "generic-riscv",
            "schema 1\ntarget generic-riscv\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imac-unknown-none-elf\n",
        );
        let target = TargetSpec::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        target.require_available_backend().unwrap();
        assert!(
            target
                .require_available_harness()
                .unwrap_err()
                .to_string()
                .contains("no platform harness")
        );
    }

    #[test]
    fn rejects_harness_selection_in_a_target_spec() {
        let path = write_spec(
            "target-harness",
            "schema 1\ntarget fixture\nharness fixture\narchitecture riscv32\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target riscv32imac-unknown-none-elf\n",
        );
        let error = TargetSpec::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(
            error
                .to_string()
                .contains("unknown target directive \"harness\"")
        );
    }

    #[test]
    fn rejects_an_architecture_abi_mismatch() {
        let path = write_spec(
            "mismatch",
            "schema 1\ntarget fixture\narchitecture arm-thumb\ncalling-convention riscv-ilp32\nendianness little\npointer-width 32\nrust-target thumbv7em-none-eabi\n",
        );
        let error = TargetSpec::load(&path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(error.to_string().contains("mismatch"));
    }

    #[test]
    fn recognizes_arm_float_abi_without_claiming_a_backend() {
        let path = write_spec(
            "arm-hardfloat",
            "schema 1\ntarget fixture\narchitecture arm-thumb\ncalling-convention aapcs32-hardfloat\nendianness little\npointer-width 32\nrust-target thumbv7em-none-eabihf\n",
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
