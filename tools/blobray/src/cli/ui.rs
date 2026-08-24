//! Process-wide diagnostic and tracing configuration.

use std::{env, fmt as std_fmt, time::Duration};

use indicatif::{FormattedDuration, HumanFloatCount, ProgressState, ProgressStyle};
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
        "{spinner:.cyan} {blobray_progress} {msg}"
    } else {
        "{spinner} {blobray_progress} {msg}"
    };
    ProgressStyle::with_template(template)
        .expect("static progress template")
        .with_key("blobray_progress", write_progress_state)
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

fn write_progress_state(state: &ProgressState, writer: &mut dyn std_fmt::Write) {
    let rate = measured_rate(state.pos(), state.per_sec());
    write_progress_fields(
        writer,
        state.elapsed(),
        state.pos(),
        state.len(),
        rate,
        rate.map(|_| state.eta()),
    );
}

fn measured_rate(position: u64, rate: f64) -> Option<f64> {
    (position > 0 && rate.is_finite() && rate > 0.0).then_some(rate)
}

fn write_progress_fields(
    writer: &mut dyn std_fmt::Write,
    elapsed: Duration,
    position: u64,
    length: Option<u64>,
    rate: Option<f64>,
    eta: Option<Duration>,
) {
    let _ = write!(writer, "[{}]", FormattedDuration(elapsed));
    let Some(length) = length else {
        return;
    };
    let _ = write!(writer, " {position:>5}/{length:<5} ");
    match (rate, eta) {
        (Some(rate), Some(eta)) => {
            let _ = write!(
                writer,
                "{}/s ETA {}",
                HumanFloatCount(rate),
                FormattedDuration(eta)
            );
        }
        _ => {
            let _ = writer.write_str("--/s ETA --:--:--");
        }
    }
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

    #[test]
    fn indeterminate_progress_reports_elapsed_time_without_inventing_a_total() {
        let mut rendered = String::new();
        write_progress_fields(
            &mut rendered,
            Duration::from_secs(65),
            7,
            None,
            Some(3.5),
            Some(Duration::from_secs(9)),
        );
        assert_eq!(rendered, "[00:01:05]");
    }

    #[test]
    fn determinate_progress_waits_for_a_measured_rate_before_estimating() {
        let mut rendered = String::new();
        write_progress_fields(
            &mut rendered,
            Duration::from_secs(2),
            0,
            Some(120),
            None,
            None,
        );
        assert_eq!(rendered, "[00:00:02]     0/120   --/s ETA --:--:--");
        assert_eq!(measured_rate(0, 42.0), None);
        assert_eq!(measured_rate(1, 0.0), None);
        assert_eq!(measured_rate(1, f64::NAN), None);
    }

    #[test]
    fn determinate_progress_reports_position_rate_and_eta() {
        let mut rendered = String::new();
        write_progress_fields(
            &mut rendered,
            Duration::from_secs(75),
            25,
            Some(100),
            Some(12.5),
            Some(Duration::from_secs(6)),
        );
        assert_eq!(rendered, "[00:01:15]    25/100   12.5/s ETA 00:00:06");
    }
}
