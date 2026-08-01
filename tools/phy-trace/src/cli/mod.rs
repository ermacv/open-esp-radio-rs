//! Command parsing and dispatch for the validator binary.

mod args;
mod commands;
mod json;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};

use crate::qualification::{qualify_esp32s31_channel, qualify_esp32s31_rf_init};
use crate::*;
use args::{Command, Invocation};

pub(crate) fn usage() {
    eprintln!(
        "usage:\n  open-esp-radio-phy-trace execute --svd PATH [--svd PATH]... --artifact PATH [--companion PATH] --symbol NAME [--concrete-only] [--timeline] [--arg VALUE] [--mmio ADDRESS=VALUE] [--read ADDRESS=VALUE] [--ram ADDRESS=VALUE] [--observe ADDRESS=LENGTH] [--max-steps COUNT]\n  open-esp-radio-phy-trace execute-compare --svd PATH [--svd PATH]... --vendor-artifact PATH [--vendor-companion PATH] --vendor-symbol NAME --rust-artifact PATH [--rust-companion PATH] --rust-symbol NAME [--compare-return] [--case NAME [--arg VALUE] [--mmio ADDRESS=VALUE] [--read ADDRESS=VALUE] [--ram ADDRESS=VALUE] [--vendor-ram-symbol ADDRESS=SYMBOL] [--rust-ram-symbol ADDRESS=SYMBOL] [--observe ADDRESS=LENGTH] [--max-steps COUNT]]...\n  open-esp-radio-phy-trace qualify-esp32s31-channel --svd PATH [--svd PATH]... --vendor-artifact PATH --vendor-companion PATH\n  open-esp-radio-phy-trace qualify-esp32s31-rf-init --svd PATH [--svd PATH]... --vendor-artifact PATH --vendor-companion PATH\n  open-esp-radio-phy-trace verify-profiles --svd PATH [--svd PATH]... --profiles PATH --vendor-artifact PATH [--vendor-companion PATH] --rust-artifact PATH [--rust-companion PATH]\n  open-esp-radio-phy-trace analyze --svd PATH [--svd PATH]... --artifact PATH [--companion PATH]... [--symbol-prefix PREFIX] [--entry-contract none|esp32s31-phy-cold|esp32s31-phy-registered] [--json-report PATH]\n  open-esp-radio-phy-trace generate-reference --svd PATH [--svd PATH]... --artifact PATH [--companion PATH]... [--member NAME] --symbol NAME [--entry-contract none|esp32s31-phy-cold|esp32s31-phy-registered] [--output PATH]\n  open-esp-radio-phy-trace generate-reference-batch --svd PATH [--svd PATH]... --artifact PATH [--companion PATH]... [--symbol-prefix PREFIX] [--probe-prefix PREFIX] [--source-name NAME] [--entry-contract none|esp32s31-phy-cold|esp32s31-phy-registered] --output-dir PATH [--manifest PATH] [--force]\n  open-esp-radio-phy-trace verify --svd PATH [--svd PATH]... --vendor-artifact PATH [--vendor-inventory PATH] --rust-artifact PATH [--profiles PATH] [--vendor-companion PATH] [--rust-companion PATH] [--vendor-prefix PREFIX] [--rust-prefix PREFIX] [--gate completion|regression] [--match-floor COUNT] [--evidence-baseline PATH]\n  open-esp-radio-phy-trace verify-all --svd PATH [--svd PATH]... --rom-artifact PATH --archive-artifact PATH --archive-inventory PATH --rust-artifact PATH [--profiles PATH] [--dispositions PATH] [--rom-companion PATH] [--archive-companion PATH] [--rust-companion PATH] [--rom-prefix PREFIX] [--archive-prefix PREFIX] [--rust-prefix PREFIX] [--gate completion|regression] [--match-floor COUNT] [--evidence-baseline PATH] [--json-report PATH]\n  open-esp-radio-phy-trace extract --svd PATH [--svd PATH]... --artifact PATH [--member NAME] --symbol NAME\n  open-esp-radio-phy-trace compare --svd PATH [--svd PATH]... --left-artifact PATH [--left-member NAME] --left-symbol NAME --right-artifact PATH [--right-member NAME] --right-symbol NAME"
    );
}

pub(crate) fn run() -> Result<bool> {
    let Invocation {
        command,
        svd_paths,
        arguments: filtered,
    } = Invocation::parse(env::args().skip(1))?;
    let svd = MmioRegisterMap::load_all(&svd_paths)?;
    commands::run(command, filtered, &svd)
}
