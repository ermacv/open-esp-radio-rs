use super::*;
use oer_ieee80211_mac::roaming::Elements;

fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 1,
    }
}
#[test]
fn link_report_can_precede_tx_result_and_old_epoch_cannot_complete_request() {
    let now = Instant::EPOCH;
    let mut owner = LinkMeasurementRequester::<32>::new(link(), Duration::from_secs(1)).unwrap();
    let id = owner
        .request(
            now,
            LinkMeasurementRequest {
                dialog_token: 7,
                transmit_power_dbm: 12,
                maximum_transmit_power_dbm: 20,
                subelements: Elements::EMPTY,
            },
        )
        .unwrap();
    let wire = [5, 3, 7, 35, 2, 10, 5, 1, 2, 100, 90, 221, 1, 8];
    assert_eq!(
        owner.receive(link(), &wire, now).unwrap(),
        RequestEvent::Ignored
    );
    owner.admitted(id, now).unwrap();
    assert_eq!(
        owner
            .receive(
                LinkIdentity {
                    generation: 0,
                    ..link()
                },
                &wire,
                now
            )
            .unwrap(),
        RequestEvent::Ignored
    );
    assert_eq!(
        owner.receive(link(), &wire, now).unwrap(),
        RequestEvent::ResponseReceived { id }
    );
    assert_eq!(owner.report().unwrap().rcpi, 100);
    assert_eq!(
        owner.tx_completed(id, TxOutcome::Failed, now).unwrap(),
        RequestEvent::Ignored
    );
}

#[test]
fn link_responder_preserves_measurements_and_requires_matching_token() {
    let mut owner = LinkMeasurementResponder::<32>::new(link(), Duration::from_secs(1)).unwrap();
    let request = owner.receive(link(), &[5, 2, 7, 12, 20]).unwrap().unwrap();
    let report = LinkMeasurementReport::parse(&[5, 3, 8, 35, 2, 10, 5, 1, 2, 100, 90]).unwrap();
    assert_eq!(
        owner.respond(Instant::EPOCH, request, report),
        Err(Error::WrongOperation)
    );
    let id = owner
        .respond(
            Instant::EPOCH,
            request,
            LinkMeasurementReport {
                dialog_token: 7,
                ..report
            },
        )
        .unwrap();
    owner.admitted(id, Instant::EPOCH).unwrap();
    assert_eq!(
        owner
            .tx_completed(id, TxOutcome::Acknowledged, Instant::EPOCH)
            .unwrap(),
        DialogEvent::Transmitted
    );
}
