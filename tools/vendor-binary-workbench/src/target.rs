//! Architecture and calling-convention selection at the workbench boundary.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Architecture {
    Riscv32,
    Xtensa,
    ArmThumb,
}

impl Architecture {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "riscv32" => Ok(Self::Riscv32),
            "xtensa" => Ok(Self::Xtensa),
            "arm-thumb" => Ok(Self::ArmThumb),
            _ => Err(format!("unsupported architecture {value:?} at line {line}").into()),
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
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "riscv-ilp32" => Ok(Self::RiscvIlp32),
            "xtensa-call0" => Ok(Self::XtensaCall0),
            "xtensa-windowed" => Ok(Self::XtensaWindowed),
            "aapcs32-softfloat" => Ok(Self::Aapcs32SoftFloat),
            "aapcs32-hardfloat" => Ok(Self::Aapcs32HardFloat),
            _ => Err(format!("unsupported calling convention {value:?} at line {line}").into()),
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
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value {
            "little" => Ok(Self::Little),
            "big" => Ok(Self::Big),
            _ => Err(format!("unsupported endianness {value:?} at line {line}").into()),
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
        let input = fs::read_to_string(path)?;
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
            let line_number = index + 1;
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (directive, value) = line
                .split_once(char::is_whitespace)
                .map(|(directive, value)| (directive, value.trim()))
                .filter(|(_, value)| !value.is_empty())
                .ok_or_else(|| format!("target directive needs a value at line {line_number}"))?;
            match directive {
                "schema" => set_once(
                    &mut schema,
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid schema at line {line_number}"))?,
                    directive,
                    line_number,
                )?,
                "target" => {
                    if value.chars().any(char::is_whitespace) {
                        return Err(
                            format!("target id must be one token at line {line_number}").into()
                        );
                    }
                    set_once(&mut id, value.to_owned(), directive, line_number)?;
                }
                "architecture" => set_once(
                    &mut architecture,
                    Architecture::parse(value, line_number)?,
                    directive,
                    line_number,
                )?,
                "calling-convention" => set_once(
                    &mut calling_convention,
                    CallingConvention::parse(value, line_number)?,
                    directive,
                    line_number,
                )?,
                "endianness" => set_once(
                    &mut endianness,
                    Endianness::parse(value, line_number)?,
                    directive,
                    line_number,
                )?,
                "pointer-width" => set_once(
                    &mut pointer_width,
                    value
                        .parse::<u8>()
                        .map_err(|_| format!("invalid pointer width at line {line_number}"))?,
                    directive,
                    line_number,
                )?,
                "rust-target" => {
                    set_token(&mut rust_target, value, directive, line_number)?;
                }
                "memory-map" => set_once(
                    &mut memory_map,
                    resolve_path(base, value),
                    directive,
                    line_number,
                )?,
                "svd" => svd_paths.push(resolve_path(base, value)),
                "pac-bindings" => set_once(
                    &mut pac_bindings,
                    resolve_path(base, value),
                    directive,
                    line_number,
                )?,
                "profiles" => set_once(
                    &mut profiles,
                    resolve_path(base, value),
                    directive,
                    line_number,
                )?,
                "dispositions" => set_once(
                    &mut dispositions,
                    resolve_path(base, value),
                    directive,
                    line_number,
                )?,
                "evidence-baseline" => set_once(
                    &mut evidence_baseline,
                    resolve_path(base, value),
                    directive,
                    line_number,
                )?,
                _ => {
                    return Err(format!(
                        "unknown target directive {directive:?} at line {line_number}"
                    )
                    .into());
                }
            }
        }

        if schema != Some(1) {
            return Err("target spec requires schema 1".into());
        }
        let target = Self {
            id: id.ok_or("target spec has no target id")?,
            harness: None,
            architecture: architecture.ok_or("target spec has no architecture")?,
            calling_convention: calling_convention
                .ok_or("target spec has no calling-convention")?,
            endianness: endianness.ok_or("target spec has no endianness")?,
            pointer_width: pointer_width.ok_or("target spec has no pointer-width")?,
            rust_target: rust_target.ok_or("target spec has no rust-target")?,
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
            return Err(format!(
                "target {} has an architecture/calling-convention mismatch",
                self.id
            )
            .into());
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
            return Err(format!(
                "target {} is valid but its architecture backend is not implemented",
                self.id
            )
            .into());
        }
        Ok(())
    }

    /// Harness selection is supplied by a reviewed platform pack.
    pub(crate) fn require_available_harness(&self) -> Result<&str> {
        let harness = self
            .harness
            .as_deref()
            .ok_or_else(|| format!("target {} has no platform harness", self.id))?;
        if !crate::harnesses::is_available(harness) {
            return Err(format!(
                "target {} selects unavailable harness {:?}",
                self.id, harness
            )
            .into());
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

fn set_once<T>(slot: &mut Option<T>, value: T, directive: &str, line: usize) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate {directive} at line {line}").into());
    }
    Ok(())
}

fn set_token(slot: &mut Option<String>, value: &str, directive: &str, line: usize) -> Result<()> {
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{directive} must be one token at line {line}").into());
    }
    set_once(slot, value.to_owned(), directive, line)
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
