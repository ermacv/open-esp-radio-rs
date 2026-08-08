//! Process-wide diagnostic and tracing configuration.

use std::io::{IsTerminal, stderr};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use super::args::{ColorMode, UiArgs};
use crate::Result;

pub(super) fn init(arguments: &UiArgs) -> Result<()> {
    let level = if arguments.quiet {
        LevelFilter::OFF
    } else {
        match arguments.verbose {
            0 => LevelFilter::WARN,
            1 => LevelFilter::INFO,
            2 => LevelFilter::DEBUG,
            _ => LevelFilter::TRACE,
        }
    };
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();
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
