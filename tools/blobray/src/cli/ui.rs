//! Process-wide diagnostic and tracing configuration.

use std::env;

use indicatif::ProgressStyle;
use tracing::level_filters::LevelFilter;
use tracing_indicatif::{
    IndicatifLayer,
    filter::{IndicatifFilter, hide_indicatif_span_fields},
};
use tracing_subscriber::{
    EnvFilter,
    fmt::{self, format::DefaultFields},
    layer::{Layer, SubscriberExt},
    util::SubscriberInitExt,
};

use super::args::UiArgs;
use crate::Result;

pub(super) fn init(arguments: &UiArgs) -> Result<()> {
    let filter = diagnostic_filter(arguments);
    let stderr_is_terminal = super::terminal::stderr_is_terminal();
    let progress_enabled = super::progress::enabled_for(arguments, stderr_is_terminal);
    let ansi = super::terminal::color_enabled(arguments.color, stderr_is_terminal);
    miette::set_hook(Box::new(move |_| {
        Box::new(miette::MietteHandlerOpts::new().color(ansi).build())
    }))?;
    let indicatif = IndicatifLayer::new()
        .with_span_field_formatter(hide_indicatif_span_fields(DefaultFields::new()))
        .with_progress_style(progress_style(ansi));
    let diagnostic_writer = indicatif.get_stderr_writer();
    let indicatif = progress_enabled.then(|| indicatif.with_filter(IndicatifFilter::new(false)));
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(ansi)
                .with_target(false)
                .with_writer(diagnostic_writer)
                .with_filter(filter),
        )
        .with(indicatif)
        .try_init()?;
    Ok(())
}

fn progress_style(ansi: bool) -> ProgressStyle {
    let template = if ansi {
        "{spinner:.cyan} {msg}"
    } else {
        "{spinner} {msg}"
    };
    ProgressStyle::with_template(template)
        .expect("static progress template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
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
    const BLOBRAY_TARGETS: &[&str] = &[
        "blobray",
        "open_radio_vendor_analysis_model",
        "open_radio_vendor_backend_riscv",
        "open_radio_vendor_semantics",
        "open_esp_radio_register_model",
    ];
    BLOBRAY_TARGETS.iter().fold(
        EnvFilter::new(LevelFilter::ERROR.to_string()),
        |filter, target| {
            filter.add_directive(
                format!("{target}={level}")
                    .parse()
                    .expect("static blobray tracing directive"),
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
        assert!(filter.contains("blobray=warn"));
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
