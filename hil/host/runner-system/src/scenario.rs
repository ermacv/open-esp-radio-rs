//! The `[system]` scenario table: radio-free workloads and their images.

use std::path::Path;

use hil_core::{
    context::Context,
    image::ImageClass,
    scenario::{Plan, bounded},
};
use serde::{Deserialize, Serialize};

use crate::{Result, workload::system};

/// One radio-free workload. Each kind implies its firmware image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SystemScenario {
    // Unit variants of an internally tagged enum would ignore unknown keys;
    // empty struct variants keep `deny_unknown_fields` effective.
    /// Bootstrap, PSRAM mapping and runtime startup.
    BootSmoke {},
    /// SoC watchdog reset on its exclusive radio-free image.
    Watchdog {},
    MemoryBenchmark {
        boots: u8,
        iterations: u16,
        sizes: Vec<u16>,
        #[serde(default = "single_frame_batch")]
        batch_sizes: Vec<u8>,
    },
    /// Target timer progress against the host clock on the correctness image.
    Timebase {
        boots: u8,
        intervals: u16,
        period_millis: u16,
    },
}

fn single_frame_batch() -> Vec<u8> {
    vec![1]
}

const BOOT_SMOKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl SystemScenario {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::BootSmoke {} | Self::Watchdog {} => Ok(()),
            Self::MemoryBenchmark {
                boots,
                iterations,
                sizes,
                batch_sizes,
            } => {
                bounded(*boots, 1, 20, "boots")?;
                bounded(*iterations, 1, 64, "iterations")?;
                bounded(sizes.len(), 1, 16, "sizes.len()")?;
                bounded(batch_sizes.len(), 1, 32, "batch_sizes.len()")?;
                for (index, size) in sizes.iter().enumerate() {
                    bounded(*size, 1, 4096, "size")?;
                    if sizes[..index].contains(size) {
                        return Err("memory benchmark sizes must be distinct".into());
                    }
                }
                for (index, frames) in batch_sizes.iter().enumerate() {
                    bounded(*frames, 1, 32, "batch size")?;
                    if batch_sizes[..index].contains(frames) {
                        return Err("memory benchmark batch sizes must be distinct".into());
                    }
                    if sizes
                        .iter()
                        .any(|bytes| u32::from(*bytes) * u32::from(*frames) > 49_152)
                    {
                        return Err(
                            "memory benchmark case exceeds 49152 payload bytes per iteration"
                                .into(),
                        );
                    }
                }
                Ok(())
            }
            Self::Timebase {
                boots,
                intervals,
                period_millis,
            } => {
                bounded(*boots, 1, 20, "boots")?;
                bounded(*intervals, 2, 100, "intervals")?;
                bounded(*period_millis, 1, 1_000, "period_millis")
            }
        }
    }

    pub fn plan(&self) -> Plan {
        Plan::target_only(match self {
            Self::BootSmoke {} => ImageClass::BootSmoke,
            Self::Watchdog {} => ImageClass::SystemWatchdog,
            Self::MemoryBenchmark { .. } => ImageClass::DiagnosticMemoryBenchmark,
            Self::Timebase { .. } => ImageClass::Correctness,
        })
    }

    pub fn run(&self, output: &Path, context: &Context<'_>) -> Result<()> {
        match self {
            Self::BootSmoke {} => context.with_capture(output, |capture| {
                capture.wait_for_boot_smoke(BOOT_SMOKE_TIMEOUT)
            }),
            Self::Watchdog {} => system::watchdog::run(output, context),
            Self::MemoryBenchmark {
                boots,
                iterations,
                sizes,
                batch_sizes,
            } => system::memory_benchmark::run(
                system::memory_benchmark::Config {
                    boots: *boots,
                    iterations: *iterations,
                    sizes,
                    batch_sizes,
                },
                output,
                context,
            ),
            Self::Timebase {
                boots,
                intervals,
                period_millis,
            } => system::timebase::run(
                system::timebase::Config {
                    boots: *boots,
                    intervals: *intervals,
                    period_millis: *period_millis,
                },
                output,
                context,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> SystemScenario {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn each_workload_implies_its_image_and_needs_no_fixture() {
        for (text, image) in [
            ("kind = 'boot-smoke'", ImageClass::BootSmoke),
            ("kind = 'watchdog'", ImageClass::SystemWatchdog),
            (
                "kind = 'memory-benchmark'\nboots = 1\niterations = 1\nsizes = [64]",
                ImageClass::DiagnosticMemoryBenchmark,
            ),
            (
                "kind = 'timebase'\nboots = 1\nintervals = 2\nperiod_millis = 10",
                ImageClass::Correctness,
            ),
        ] {
            let scenario = parse(text);
            scenario.validate().unwrap();
            assert_eq!(scenario.plan(), Plan::target_only(image));
        }
    }

    #[test]
    fn memory_benchmark_rejects_duplicate_or_oversized_cases() {
        for text in [
            "kind = 'memory-benchmark'\nboots = 1\niterations = 1\nsizes = [64, 64]",
            "kind = 'memory-benchmark'\nboots = 1\niterations = 1\nsizes = []",
            "kind = 'memory-benchmark'\nboots = 1\niterations = 1\nsizes = [4096]\nbatch_sizes = [32]",
            "kind = 'memory-benchmark'\nboots = 1\niterations = 1\nsizes = [64]\nbatch_sizes = [2, 2]",
        ] {
            assert!(parse(text).validate().is_err(), "{text}");
        }
        assert!(
            toml::from_str::<SystemScenario>("kind = 'boot-smoke'\nimage = 'correctness'").is_err()
        );
    }
}
