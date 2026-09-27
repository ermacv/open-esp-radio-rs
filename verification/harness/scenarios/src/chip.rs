//! The chip a scenario binary compares.
//!
//! Each chip's scenario crate owns its pins, ROM facts, stack placement and
//! reviewed decisions, and installs them once before any scenario runs. The
//! engine reads them from here and holds no chip facts of its own.
use std::sync::OnceLock;

/// One chip's scenario configuration.
pub struct Chip {
    /// Chip identifier, such as `esp32s31`.
    pub name: &'static str,
    /// The tracked artifact manifest, relative to the repository root.
    pub manifest: &'static str,
    /// Its text, compiled into the scenario binary.
    pub manifest_text: &'static str,
    /// Production hardware sources the observation analysis attributes
    /// executed lines to, relative to the repository root.
    pub hardware_scope: &'static str,
    /// Session input index of the authenticated ROM ELF.
    pub rom_input: u64,
    /// ROM storage the scenarios address directly, as (symbol, address,
    /// size), checked against the captured ROM inventory.
    pub rom_symbols: &'static [(&'static str, u32, u64)],
    /// Caller-owned stack of every compared call, as (address, bytes).
    pub stack: (u32, u32),
    /// Reviewed decisions on unobserved production lines.
    pub observation: &'static [crate::observation::Decision],
    /// Reviewed decisions on unprojected vendor state.
    pub state: &'static [crate::state::Decision],
}

static CHIP: OnceLock<&'static Chip> = OnceLock::new();

/// Install `chip` for this process; installing the same chip again is a
/// no-op, and a different one is refused.
pub fn install(chip: &'static Chip) {
    let installed = CHIP.get_or_init(|| chip);
    assert!(
        std::ptr::eq(*installed, chip),
        "scenario engine already runs for {}",
        installed.name
    );
}

/// The installed chip.
pub fn chip() -> &'static Chip {
    CHIP.get()
        .copied()
        .expect("the scenario crate installs its chip before running scenarios")
}

/// A chip for the engine's own tests, whose hardware scope is a test tree.
#[cfg(test)]
pub(crate) static TEST: Chip = Chip {
    name: "test",
    manifest: "verification/test/artifacts.toml",
    manifest_text: "schema = 1\nsource = []\nartifact = []\n",
    hardware_scope: "crates/hardware/test",
    rom_input: 1,
    rom_symbols: &[],
    stack: (0, 0),
    observation: &[],
    state: &[],
};
