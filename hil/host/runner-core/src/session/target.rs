//! Laboratory inputs and per-scenario initialization the target protocol needs.

use oer_hil_scenario::Settings;

use crate::lab::config::LabConfig;

/// What a session needs from its repetition: the laboratory and the
/// selected scenario's target initialization settings.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub lab: &'a LabConfig,
    pub settings: Settings,
}
