//! Generic Rust PAC generation from the clean materialized SVD.

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use svd2rust::{
    Target,
    config::{Config, RustEdition},
};

use crate::{Result, registers::PacApiPack};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PacTarget {
    None,
    Riscv,
}

impl PacTarget {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "riscv" => Ok(Self::Riscv),
            _ => Err(format!("PAC target must be \"none\" or \"riscv\", got {value:?}").into()),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Riscv => "riscv",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PacEdition {
    E2021,
    E2024,
}

impl PacEdition {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "2021" => Ok(Self::E2021),
            "2024" => Ok(Self::E2024),
            _ => Err(format!("PAC edition must be \"2021\" or \"2024\", got {value:?}").into()),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::E2021 => "2021",
            Self::E2024 => "2024",
        }
    }
}

#[tracing::instrument(
    name = "render_pac_source",
    skip_all,
    fields(target = target.label(), edition = edition.label(), reviewed_api = api.is_some())
)]
pub(crate) fn generate_pac_with_api(
    svd: &str,
    target: PacTarget,
    edition: PacEdition,
    api: Option<&PacApiPack>,
) -> Result<String> {
    let mut config = Config::default();
    config.target = match target {
        PacTarget::None => Target::None,
        PacTarget::Riscv => Target::RISCV,
    };
    config.edition = match edition {
        PacEdition::E2021 => RustEdition::E2021,
        PacEdition::E2024 => RustEdition::E2024,
    };
    config.strict = true;
    let mut source = svd2rust::generate(svd, &config)
        .map_err(|error| format!("svd2rust generation failed: {error}"))?
        .lib_rs;
    if api.is_some_and(|api| api.options.allow_clippy_empty_docs) {
        source.insert_str(0, "#![allow(clippy::empty_docs)]\n");
    }
    if let Some(api) = api {
        source.push_str(&api.render_rust(svd)?);
    }
    format_generated(&source, edition)
}

fn format_generated(source: &str, edition: PacEdition) -> Result<String> {
    let mut child = Command::new("rustfmt")
        .args([
            "--edition",
            edition.label(),
            "--style-edition",
            edition.label(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start rustfmt for generated PAC: {error}"))?;
    child
        .stdin
        .take()
        .expect("piped rustfmt stdin must exist")
        .write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt failed for generated PAC: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_formatted_architecture_neutral_pac() {
        let svd = r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance" xs:noNamespaceSchemaLocation="CMSIS-SVD.xsd">
  <name>TEST_DEVICE</name>
  <version>0.1</version>
  <description>Test device</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>RADIO</name>
      <baseAddress>0x1000</baseAddress>
      <registers>
        <register>
          <name>CONTROL</name>
          <addressOffset>0</addressOffset>
          <size>32</size>
          <access>read-write</access>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;
        let source = generate_pac_with_api(svd, PacTarget::None, PacEdition::E2024, None).unwrap();
        assert!(source.contains("pub mod radio"));
        assert!(source.contains("pub struct Peripherals"));
    }
}
