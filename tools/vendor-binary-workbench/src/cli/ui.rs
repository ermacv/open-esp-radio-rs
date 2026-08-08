//! Process-wide diagnostic and tracing configuration.

use std::{
    env,
    io::{IsTerminal, stderr},
};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use super::args::{ColorMode, UiArgs};
use crate::Result;

pub(super) fn init(arguments: &UiArgs) -> Result<()> {
    let filter = diagnostic_filter(arguments);
    let ansi = match arguments.color {
        ColorMode::Auto => stderr().is_terminal(),
        ColorMode::Always => true,
        ColorMode::Never => false,
    };
    miette::set_hook(Box::new(move |_| {
        Box::new(miette::MietteHandlerOpts::new().color(ansi).build())
    }))?;
    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_ansi(ansi)
                .with_target(false)
                .with_writer(stderr),
        )
        .try_init()?;
    Ok(())
}

fn diagnostic_filter(arguments: &UiArgs) -> EnvFilter {
    if arguments.quiet {
        return EnvFilter::new(LevelFilter::OFF.to_string());
    }
    if env::var_os("RUST_LOG").is_some() {
        return EnvFilter::builder()
            .with_default_directive(LevelFilter::ERROR.into())
            .from_env_lossy();
    }
    let level = match arguments.verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    default_diagnostic_filter(level)
}

fn default_diagnostic_filter(level: LevelFilter) -> EnvFilter {
    const WORKBENCH_TARGETS: &[&str] = &[
        "open_radio_vendor_binary_workbench",
        "open_radio_vendor_analysis_model",
        "open_radio_vendor_backend_riscv",
        "open_radio_vendor_semantics",
        "open_radio_vendor_harness_esp32s31",
        "open_radio_vendor_harness_esp32s31_semantic",
        "open_esp_radio_register_model",
    ];
    WORKBENCH_TARGETS.iter().fold(
        EnvFilter::new(LevelFilter::ERROR.to_string()),
        |filter, target| {
            filter.add_directive(
                format!("{target}={level}")
                    .parse()
                    .expect("static workbench tracing directive"),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_keeps_dependency_warnings_below_the_surface() {
        let filter = default_diagnostic_filter(LevelFilter::WARN).to_string();
        assert!(filter.contains("open_radio_vendor_binary_workbench=warn"));
        assert!(filter.contains("error"));
    }

    #[test]
    fn quiet_disables_diagnostics_without_consulting_verbosity() {
        let filter = diagnostic_filter(&UiArgs {
            quiet: true,
            ..UiArgs::default()
        });
        assert_eq!(filter.to_string(), "off");
    }
}
