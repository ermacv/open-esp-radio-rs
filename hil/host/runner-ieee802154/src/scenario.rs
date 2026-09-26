//! The `[ieee802154]` scenario table: IEEE 802.15.4 diagnostics.

use std::path::Path;

use hil_core::{
    context::Context,
    image::ImageClass,
    scenario::{Plan, bounded},
};
use serde::{Deserialize, Serialize};

use crate::{Result, workload::ieee802154};

/// One IEEE 802.15.4 diagnostic. Each kind implies its diagnostic image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Ieee802154Scenario {
    EventStatus(Diagnostic),
    EdEvent(Diagnostic),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub boots: u8,
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl Ieee802154Scenario {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::EventStatus(diagnostic) | Self::EdEvent(diagnostic) => *diagnostic,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let diagnostic = self.diagnostic();
        bounded(diagnostic.boots, 1, 20, "boots")?;
        bounded(diagnostic.poll_limit, 1, 1_000_000, "poll_limit")?;
        bounded(diagnostic.timer_threshold, 1, 1_000, "timer_threshold")
    }

    pub fn plan(&self) -> Plan {
        Plan::target_only(match self {
            Self::EventStatus(_) => ImageClass::DiagnosticIeee802154EventStatus,
            Self::EdEvent(_) => ImageClass::DiagnosticIeee802154EdEvent,
        })
    }

    pub fn run(&self, output: &Path, context: &Context<'_>) -> Result<()> {
        let Diagnostic {
            boots,
            poll_limit,
            timer_threshold,
        } = self.diagnostic();
        match self {
            Self::EventStatus(_) => ieee802154::event_status::run(
                ieee802154::event_status::Config {
                    boots,
                    poll_limit,
                    timer_threshold,
                },
                output,
                context,
            ),
            Self::EdEvent(_) => ieee802154::ed_event::run(
                ieee802154::ed_event::Config {
                    boots,
                    poll_limit,
                    timer_threshold,
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

    #[test]
    fn diagnostics_imply_their_images_and_bound_their_polling() {
        for (kind, image) in [
            ("event-status", ImageClass::DiagnosticIeee802154EventStatus),
            ("ed-event", ImageClass::DiagnosticIeee802154EdEvent),
        ] {
            let scenario: Ieee802154Scenario = toml::from_str(&format!(
                "kind = '{kind}'\nboots = 1\npoll_limit = 10\ntimer_threshold = 2"
            ))
            .unwrap();
            scenario.validate().unwrap();
            assert_eq!(scenario.plan(), Plan::target_only(image));
            let excessive: Ieee802154Scenario = toml::from_str(&format!(
                "kind = '{kind}'\nboots = 1\npoll_limit = 0\ntimer_threshold = 2"
            ))
            .unwrap();
            assert!(excessive.validate().is_err());
        }
    }
}
