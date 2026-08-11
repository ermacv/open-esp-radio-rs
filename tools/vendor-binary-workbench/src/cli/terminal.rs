//! Shared terminal-capability policy for stdout and stderr frontends.

use std::{env, io::IsTerminal as _};

use super::args::ColorMode;

pub(super) fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

pub(super) fn stderr_is_terminal() -> bool {
    std::io::stderr().is_terminal()
}

pub(super) fn color_enabled(mode: ColorMode, is_terminal: bool) -> bool {
    resolve_color(
        mode,
        is_terminal,
        env::var_os("NO_COLOR").is_some(),
        env::var("TERM").ok().as_deref(),
    )
}

pub(super) fn stdout_width(is_terminal: bool) -> usize {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 40)
        .or_else(|| {
            is_terminal
                .then(crossterm::terminal::size)
                .and_then(|result| result.ok())
                .map(|(width, _)| usize::from(width))
                .filter(|width| *width >= 40)
        })
        .unwrap_or(100)
}

fn resolve_color(mode: ColorMode, is_terminal: bool, no_color: bool, term: Option<&str>) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => is_terminal && !no_color && term != Some("dumb"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_color_requires_a_capable_terminal() {
        assert!(resolve_color(ColorMode::Auto, true, false, Some("xterm")));
        assert!(!resolve_color(ColorMode::Auto, false, false, Some("xterm")));
        assert!(!resolve_color(ColorMode::Auto, true, true, Some("xterm")));
        assert!(!resolve_color(ColorMode::Auto, true, false, Some("dumb")));
    }

    #[test]
    fn explicit_color_overrides_terminal_environment() {
        assert!(resolve_color(ColorMode::Always, false, true, Some("dumb")));
        assert!(!resolve_color(ColorMode::Never, true, false, Some("xterm")));
    }
}
