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
    /// Preserve and retrieve verified HIL evidence without accessing hardware.
    Archive {
        #[command(subcommand)]
        command: hil_core::archive::Command,
    },
    /// Check the selected scenarios' tools and fixture, without resetting hardware.
    Doctor(Selection),
    /// Apply and verify a scenario's fixture, then restore it. Never accesses the DUT.
    Fixture {
        #[command(subcommand)]
        command: FixtureCommand,
    },
    /// Resolve scenario requirements offline, without opening a device or lab config.
    Plan {
        #[command(flatten)]
        selection: Selection,
        /// Save the executable plan. Planning never accesses the lab or DUT.
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "upstream-xarxa")]
        network: hil_core::image::Integration,
        /// Require every named check; controls are added only after this filter.
        #[arg(long = "proof")]
        proofs: Vec<String>,
        /// Select missing obligations using the independent qualification evaluator.
        #[arg(long, conflicts_with_all = ["scenario", "tag", "proofs"])]
        qualification: Option<PathBuf>,
        #[arg(long, requires = "qualification")]
        capability: Option<String>,
    },
    /// Execute an exact saved plan after validating its current scenario inputs.
    RunPlan {
        plan: PathBuf,
        /// Explicit nonignored untracked source file; repeat for each included file.
        #[arg(long = "source-include", value_name = "FILE", conflicts_with = "check")]
        source_include: Vec<String>,
        /// Validate the saved plan offline without acquiring fixtures or a DUT.
        #[arg(long)]
        check: bool,
    },
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
        /// Explicit nonignored untracked source file; repeat for each included file.
        #[arg(
            long = "source-include",
            value_name = "FILE",
            conflicts_with = "firmware_from"
        )]
        source_include: Vec<String>,
        /// Standalone AP: RR or deficit with the same HT/OFDM24 response-envelope model (3000-us quantum).
        #[arg(long, value_enum)]
        ap_scheduler: Option<ApScheduler>,
        /// Use the scenario's image class from a sealed earlier HIL run.
        #[arg(long, value_name = "RUN_ID")]
        firmware_from: Option<String>,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(
            long,
            default_value = "upstream-xarxa",
            conflicts_with = "firmware_from"
        )]
        network: hil_core::image::Integration,
    },
    /// Execute catalog scenarios, flashing once per selected image class.
    RunAll {
        /// Explicit nonignored untracked source file; repeat for each included file.
        #[arg(long = "source-include", value_name = "FILE")]
        source_include: Vec<String>,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long, default_value = "upstream-xarxa")]
        network: hil_core::image::Integration,
        /// Select only scenarios carrying this tag. May be repeated.
        #[arg(long)]
        tag: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum ApScheduler {
    Rr,
    Deficit,
}

impl From<ApScheduler> for oer_hil_protocol::WifiApScheduler {
    fn from(value: ApScheduler) -> Self {
        match value {
            ApScheduler::Rr => Self::RrHtResponse24,
            ApScheduler::Deficit => Self::DeficitHtResponse24,
        }
    }
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
        catalog: &'a hil_core::scenario::Catalog,
    ) -> crate::Result<Vec<&'a hil_core::scenario::Scenario>> {
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
    /// Capture exact source inputs offline. Does not build, flash or qualify firmware.
    Snapshot {
        /// Explicit untracked file: repository-relative path or ROLE:path for a local override.
        #[arg(long = "source-include", value_name = "FILE")]
        source_include: Vec<String>,
    },
    /// Capture rustc mono estimates and the exact diagnostic ELF; never flash.
    Mono { class: hil_core::image::ImageClass },
    Build {
        class: hil_core::image::ImageClass,
        /// Build only from a verified source snapshot directory, not the live checkout.
        #[arg(long)]
        source_snapshot: Option<PathBuf>,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long, default_value = "upstream-xarxa")]
        network: hil_core::image::Integration,
    },
    /// Build one clean commit in two different checkout roots and compare every firmware subject.
    VerifyRebuild {
        class: hil_core::image::ImageClass,
        /// Diagnose Cargo's experimental object-path sanitization without changing normal builds.
        #[arg(long)]
        trim_paths: bool,
    },
    Flash {
        class: hil_core::image::ImageClass,
        /// Network implementation: upstream-xarxa, patched-xarxa, upstream-smoltcp or owned-xarxa.
        #[arg(long, default_value = "upstream-xarxa")]
        network: hil_core::image::Integration,
    },
    /// Verify and flash an exact application archived by an earlier HIL run.
    Replay {
        run_id: String,
        class: hil_core::image::ImageClass,
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

#[derive(Debug, Subcommand)]
pub(crate) enum FixtureCommand {
    /// Print the finite probe workload without touching any fixture.
    ProbePlan,
    /// Execute a bounded DTM command check and restore the Linux adapter.
    BluetoothCheck {
        #[arg(long, default_value = "hci0")]
        adapter: hil_bluetooth::fixture::bluetooth::model::Adapter,
        /// v1 is an explicit diagnostic; production RF scenarios always use v2.
        #[arg(long, value_enum, default_value = "v2")]
        dtm_version: hil_bluetooth::fixture::bluetooth::model::DtmVersion,
    },
    /// Connect to a public LE peer, reset the adapter and archive restoration evidence.
    BluetoothConnectReset {
        #[arg(long, default_value = "hci0")]
        adapter: hil_bluetooth::fixture::bluetooth::model::Adapter,
        #[arg(long)]
        peer: hil_bluetooth::fixture::bluetooth::model::PeerAddress,
        #[arg(long, default_value = "0", value_parser = clap::value_parser!(u16).range(0..=5000))]
        hold_ms: u16,
    },
    /// Build the pinned hostapd with explicit HIL coexistence policy support.
    BuildHostapd,
    /// Prepare and install one versioned Linux fixture software bundle.
    Install {
        /// Finite Linux fixture provider to provision.
        #[arg(long, value_enum)]
        provider: oer_hil_fixture_install::Provider,
        /// Print the offline plan without executing any installer step.
        #[arg(long)]
        dry_run: bool,
        /// Bluetooth adapter admitted by the installed policy; defaults to hci0.
        #[arg(long, value_name = "hciN")]
        adapter: Vec<String>,
    },
    /// Validate prerequisites and exercise AP preparation/restoration without firmware or serial.
    Check { scenario: String },
}
