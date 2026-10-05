//! Fixture prerequisites shared by doctor, execution and fixture-only checks.

use crate::{
    Result,
    scenario::{Families, Scenario},
};
use oer_hil_lab::config::LabConfig;
use oer_hil_run_bundle::run::{Failure, FailureKind};
use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_workload::family::Registry as _;

/// The laboratory checks a scenario needs before it may run: its
/// preconditions, then every fixture provider's check of its requirements.
pub(crate) fn check(lab: &LabConfig, scenario: &Scenario) -> Result<()> {
    let plan = scenario.plan();
    let resolved = lab.resolve(plan.wifi);
    let lab = &resolved;
    if let Some(failure) = scenario_precondition(lab, scenario) {
        return Err(oer_hil_lab::Error::new(failure.message).into());
    }
    for provider in Families::FIXTURES {
        provider.check(lab, &plan)?;
    }
    Ok(())
}

/// Refuse a peer board whose newest journaled flash is another image than
/// `expected`. A board without a journaled flash passes; the consumer's own
/// handshake decides.
fn require_peer_image(
    lab: &LabConfig,
    peer: &oer_hil_lab::config::PeerBoardConfig,
    expected: &str,
    reflash: &str,
) -> crate::Result<()> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?.with_stand_file(lab.path().to_owned());
    other_image(arbiter.latest_flash(&peer.mac)?.as_ref(), expected).map_or(
        Ok(()),
        |(image, owner)| {
            Err(format!(
                "board {} carries `{image}` flashed by {owner}, not `{expected}`; {reflash}",
                lab.stand().label(&peer.mac)
            )
            .into())
        },
    )
}

/// The image and its flasher when `latest` is a flash of another image than
/// `expected`.
fn other_image(
    latest: Option<&oer_hil_arbiter::BoardEvent>,
    expected: &str,
) -> Option<(String, String)> {
    match latest.map(|event| (&event.kind, &event.owner)) {
        Some((oer_hil_arbiter::BoardEventKind::Flashed { image, .. }, owner))
            if image != expected =>
        {
            Some((image.clone(), owner.clone()))
        }
        _ => None,
    }
}

/// What a scenario needs of the laboratory before it may run: its family's
/// precondition, the peer image it uses on the peer board, and each fixture
/// provider's precondition of its plan. A failure blocks the scenario.
pub(crate) fn scenario_precondition(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    let precondition = |error: Box<dyn std::error::Error + Send + Sync>| {
        Some(Failure::new(FailureKind::Precondition, error.to_string()))
    };
    if let Err(error) = selected.family.precondition(lab) {
        return precondition(error);
    }
    if let Some(image) = selected.family.peer_image()
        && let Err(error) = lab
            .peer()
            .and_then(|peer| require_peer_image(lab, &peer, image.name, image.reflash))
    {
        return precondition(error);
    }
    let plan = selected.plan();
    Families::FIXTURES
        .iter()
        .find_map(|provider| provider.precondition(lab, &plan).err())
        .and_then(precondition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_is_refused_only_when_its_newest_flash_is_another_image() {
        let flash = |image: &str| oer_hil_arbiter::BoardEvent {
            unix: 1,
            owner: String::from("bluetooth"),
            checkout: None,
            device: Some(String::from("38:44:BE:AA:25:64")),
            kind: oer_hil_arbiter::BoardEventKind::Flashed {
                image: image.to_owned(),
                application_sha256: String::from("ab"),
                commit: None,
                dirty: None,
                origin: String::from("test"),
            },
        };
        assert_eq!(other_image(None, "peer"), None);
        assert_eq!(other_image(Some(&flash("peer")), "peer"), None);
        assert_eq!(
            other_image(Some(&flash("ble-peer")), "peer"),
            Some((String::from("ble-peer"), String::from("bluetooth")))
        );
    }
}
