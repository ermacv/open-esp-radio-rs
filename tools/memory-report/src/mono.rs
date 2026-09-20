//! Import rustc `-Zdump-mono-stats-format=json` estimates without attributing
//! them to final ELF bytes. Inputs from distinct compilations remain separate.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Compiler estimate units, not bytes of the linked firmware.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MonoDefinition {
    pub name: String,
    pub instantiation_count: u64,
    pub size_estimate: u64,
    pub total_estimate: u64,
}

#[derive(Debug, Serialize)]
pub struct MonoReport {
    pub schema: u32,
    pub input: PathBuf,
    pub definitions: Vec<MonoDefinition>,
}

/// Reads one compiler output. No inference of commit, target, LTO mode or
/// firmware identity is made from its filename. Preserve build provenance
/// with the input when comparing compilations.
pub fn analyze_mono(path: &Path) -> Result<MonoReport> {
    let data = fs::read(path).map_err(|source| Error::Read {
        path: path.into(),
        source,
    })?;
    let mut definitions: Vec<MonoDefinition> = serde_json::from_slice(&data)?;
    definitions.sort_by(|a, b| {
        b.total_estimate
            .cmp(&a.total_estimate)
            .then(a.name.cmp(&b.name))
    });
    Ok(MonoReport {
        schema: 1,
        input: path.into(),
        definitions,
    })
}

pub fn render_mono_report(report: &MonoReport) -> String {
    use std::fmt::Write;
    let mut out = format!(
        "Compiler mono estimates from {} (not linked bytes)\n",
        report.input.display()
    );
    for d in &report.definitions {
        writeln!(
            out,
            "{} instances; estimate {} each / {} total: {}",
            d.instantiation_count, d.size_estimate, d.total_estimate, d.name
        )
        .unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_compiler_json_preserves_estimates_without_byte_conversion() {
        // Shape checked using the repository's pinned rustc and a two-type generic.
        let values: Vec<MonoDefinition> = serde_json::from_str(
            r#"[{"name":"identity","instantiation_count":2,"size_estimate":2,"total_estimate":4}]"#,
        )
        .unwrap();
        assert_eq!(values[0].instantiation_count, 2);
        assert_eq!(values[0].total_estimate, 4);
    }
    #[test]
    fn changed_or_incomplete_compiler_format_is_not_silently_accepted() {
        for input in [
            r#"[{"name":"x"}]"#,
            r#"[{"name":"x","instantiation_count":1,"size_estimate":1,"total_estimate":1,"bytes":1}]"#,
        ] {
            assert!(serde_json::from_str::<Vec<MonoDefinition>>(input).is_err());
        }
    }
}
