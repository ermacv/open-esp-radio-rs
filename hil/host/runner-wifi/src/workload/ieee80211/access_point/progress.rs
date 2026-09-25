//! Preserve independent observations before interpreting qualification gates.

use serde::Serialize;
use std::{collections::BTreeMap, fmt::Display, fs, path::Path};

#[derive(Serialize)]
pub(super) struct CycleProgress {
    schema: u8,
    cycle: u8,
    stages: BTreeMap<&'static str, serde_json::Value>,
}

impl CycleProgress {
    pub(super) fn new(cycle: u8) -> Self {
        Self {
            schema: 1,
            cycle,
            stages: BTreeMap::new(),
        }
    }

    pub(super) fn record<T: Serialize, E: Display>(
        &mut self,
        stage: &'static str,
        result: &Result<T, E>,
    ) {
        let value = match result {
            Ok(value) => serde_json::json!({"status": "available", "value": value}),
            Err(error) => serde_json::json!({"status": "error", "error": error.to_string()}),
        };
        self.stages.insert(stage, value);
    }

    pub(super) fn save(&self, output: &Path) -> crate::Result<()> {
        fs::create_dir_all(output)?;
        fs::write(
            output.join("cycle-progress.json"),
            serde_json::to_vec_pretty(self)?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
