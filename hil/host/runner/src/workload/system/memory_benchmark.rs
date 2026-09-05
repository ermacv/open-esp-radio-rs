//! Pre-initialization copy diagnostics with preserved partial observations.

use crate::{Result, execution::context::Context};
use open_esp_radio_hil_protocol::{
    MemoryBenchmarkEvidence, MemoryBenchmarkMode, MemoryBenchmarkRequest, MemoryBenchmarkSource,
    MemoryBenchmarkStop,
};
use serde::Serialize;
use std::{fs, path::Path, time::Duration};

pub(crate) struct Config<'a> {
    pub(crate) boots: u8,
    pub(crate) iterations: u16,
    pub(crate) sizes: &'a [u16],
    pub(crate) batch_sizes: &'a [u8],
}

#[derive(Serialize)]
struct CaseReport {
    boot: u8,
    requested: MemoryBenchmarkRequest,
    target: MemoryBenchmarkEvidence,
}

pub(crate) fn run(config: Config<'_>, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut reports = Vec::new();
    let requests = requests(&config);
    for boot in 1..=config.boots {
        let result = context.with_capture(&output.join(format!("boot-{boot:03}")), |capture| {
            let capabilities = capture.request_capabilities(Duration::from_secs(10))?;
            if !capabilities.features.memory_benchmark {
                return Err("firmware does not advertise memory benchmarks".into());
            }
            for &requested in &requests {
                let target = capture.probe_memory_benchmark(requested, Duration::from_secs(15))?;
                reports.push(CaseReport {
                    boot,
                    requested,
                    target,
                });
                // Persist the terminal target observation before evaluating it.
                write_report(output, &reports, "incomplete", None)?;
                validate(requested, target)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            write_report(output, &reports, "failed", Some(&error.to_string()))?;
            return Err(error);
        }
    }
    write_report(output, &reports, "passed", None)?;
    eprintln!("memory_benchmark=PASS cases={}", reports.len());
    Ok(())
}

fn requests(config: &Config<'_>) -> Vec<MemoryBenchmarkRequest> {
    let mut requests = Vec::with_capacity(config.sizes.len() * config.batch_sizes.len() * 6);
    for source in [MemoryBenchmarkSource::Sram, MemoryBenchmarkSource::Psram] {
        for &bytes in config.sizes {
            for &frames in config.batch_sizes {
                for mode in [
                    MemoryBenchmarkMode::CpuCopy,
                    MemoryBenchmarkMode::GdmaBlocking,
                    MemoryBenchmarkMode::GdmaAsync,
                ] {
                    requests.push(MemoryBenchmarkRequest {
                        mode,
                        source,
                        bytes,
                        frames,
                        iterations: config.iterations,
                    });
                }
            }
        }
    }
    requests
}

fn write_report(
    output: &Path,
    reports: &[CaseReport],
    status: &str,
    failure: Option<&str>,
) -> Result<()> {
    fs::write(
        output.join("memory-benchmark.json"),
        serde_json::to_vec_pretty(&report_document(reports, status, failure))?,
    )?;
    Ok(())
}

fn report_document(
    reports: &[CaseReport],
    status: &str,
    failure: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "schema": 2,
        "status": status,
        "failure": failure,
        "counter_scopes": {
            "elapsed": "measuring-hart interval, including other activity between async polls",
            "foreground": "whole CPU/blocking operation; async prepare/start, polls and cleanup; includes IRQs inside those windows",
        },
        "cpu_utilization_measured": false,
        "cases": reports,
    })
}

fn validate(request: MemoryBenchmarkRequest, target: MemoryBenchmarkEvidence) -> Result<()> {
    if !request.validate() || target.request != request {
        return Err("memory benchmark response does not match its bounded request".into());
    }
    if target.stop != MemoryBenchmarkStop::Completed {
        return Err(format!("memory benchmark stopped at {:?}: {target:?}", target.stop).into());
    }
    if target.completed_iterations != request.iterations {
        return Err("memory benchmark did not complete every requested iteration".into());
    }
    if target.elapsed_cycles == 0
        || target.elapsed_instructions == 0
        || target.foreground_cycles == 0
        || target.foreground_instructions == 0
        || target.foreground_cycles > target.elapsed_cycles
        || target.foreground_instructions > target.elapsed_instructions
    {
        return Err("memory benchmark counter scopes are inconsistent".into());
    }
    match request.mode {
        MemoryBenchmarkMode::GdmaAsync if target.polls < u32::from(request.iterations) => {
            Err("async memory benchmark did not poll every transfer".into())
        }
        MemoryBenchmarkMode::CpuCopy | MemoryBenchmarkMode::GdmaBlocking
            if target.polls != 0
                || target.foreground_cycles != target.elapsed_cycles
                || target.foreground_instructions != target.elapsed_instructions =>
        {
            Err("synchronous memory benchmark counter scope is not the whole operation".into())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
