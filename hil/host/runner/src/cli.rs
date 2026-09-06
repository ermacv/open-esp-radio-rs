//! Public command grammar; workload APIs accept typed configuration.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "cargo hil",
    about = "Open ESP radio hardware-in-the-loop runner"
)]
pub(crate) struct Cli {
    /// Complete local fixture configuration. Secrets never belong to scenarios.
    #[arg(long, global = true)]
    pub(crate) lab_config: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: CliCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CliCommand {
    /// Check the selected scenarios' tools and fixture, without resetting hardware.
    Doctor(Selection),
    /// Resolve scenario requirements offline, without opening a device or lab config.
    Plan(Selection),
    /// Inspect and validate the host-owned scenario catalog.
    Scenario {
        #[command(subcommand)]
        command: ScenarioCommand,
    },
    /// Build or flash one reproducible firmware class.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
    /// Inspect the currently flashed target without starting a workload.
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Rebuild derived views from immutable run bundles.
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Build, flash and execute one catalog scenario.
    Run {
        scenario: String,
        /// Use the scenario's image class from a sealed earlier HIL run.
        #[arg(long, value_name = "RUN_ID")]
        firmware_from: Option<String>,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(
            long,
            default_value = "upstream-xarxa",
            conflicts_with = "firmware_from"
        )]
        network: crate::image::Integration,
    },
    /// Execute catalog scenarios, flashing once per selected image class.
    RunAll {
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long, default_value = "upstream-xarxa")]
        network: crate::image::Integration,
        /// Select only scenarios carrying this tag. May be repeated.
        #[arg(long)]
        tag: Vec<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct Selection {
    /// One scenario; omission selects the catalog, optionally filtered by tags.
    #[arg(conflicts_with = "tag")]
    pub(crate) scenario: Option<String>,
    /// Require every supplied tag. May be repeated.
    #[arg(long)]
    pub(crate) tag: Vec<String>,
}

impl Selection {
    pub(crate) fn resolve<'a>(
        &self,
        catalog: &'a crate::scenario::Catalog,
    ) -> crate::Result<Vec<&'a crate::scenario::Scenario>> {
        if let Some(id) = &self.scenario {
            return Ok(vec![catalog.get(id)?]);
        }
        let selected: Vec<_> = catalog
            .all()
            .iter()
            .filter(|entry| self.tag.iter().all(|tag| entry.tags.contains(tag)))
            .collect();
        if selected.is_empty() {
            return Err("no HIL scenarios match the requested tags".into());
        }
        Ok(selected)
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum ScenarioCommand {
    List,
    Validate { scenario: Option<String> },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ImageCommand {
    Build {
        class: crate::image::ImageClass,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long, default_value = "upstream-xarxa")]
        network: crate::image::Integration,
    },
    /// Build one clean commit in two different checkout roots and compare every firmware subject.
    VerifyRebuild {
        class: crate::image::ImageClass,
        /// Diagnose Cargo's experimental object-path sanitization without changing normal builds.
        #[arg(long)]
        trim_paths: bool,
    },
    Flash {
        class: crate::image::ImageClass,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long, default_value = "upstream-xarxa")]
        network: crate::image::Integration,
    },
    /// Verify and flash an exact application archived by an earlier HIL run.
    Replay {
        run_id: String,
        class: crate::image::ImageClass,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeviceCommand {
    Status,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ReportCommand {
    /// Rebuild history.json and history.html without attached hardware.
    Rebuild,
    /// Verify one run bundle, or every bundle when RUN_ID is omitted.
    Verify { run_id: Option<String> },
}

#[cfg(test)]
mod tests;
