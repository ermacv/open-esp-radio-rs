use super::*;
use gas::{AdvertisementProtocol, Body, Fragment, Frame, ProtocolId, ResponseInfo, Status};
use oer_ieee80211_mac::sequence::SequenceNumber;
use requester::{Outcome, Progress};
use responder::Event;

type Client = requester::Requester<128, 512>;
type Server = responder::Responder<2, 64, 512, 128>;
fn now(micros: u64) -> Instant {
    Instant::from_micros(micros)
}
fn peer() -> PeerIdentity {
    PeerIdentity {
        local: [2, 0, 0, 0, 0, 1],
        peer: [2, 0, 0, 0, 0, 2],
        bssid: [2, 0, 0, 0, 0, 2],
        epoch: 1,
    }
}
fn remote() -> PeerIdentity {
    PeerIdentity {
        local: peer().peer,
        peer: peer().local,
        ..peer()
    }
}
fn config() -> requester::Config {
    requester::Config {
        dialog_timeout: Duration::from_millis(20),
        response_timeout: Duration::from_millis(3),
        max_restarts: 2,
        token_reuse_guard: Duration::from_millis(40),
    }
}
fn server_config(fragment: usize) -> responder::Config {
    responder::Config {
        dialog_timeout: Duration::from_millis(40),
        fragment_payload_limit: fragment,
        comeback_delay_tu: 1,
        response_info: ResponseInfo::new(127, false).unwrap(),
    }
}
fn client() -> Client {
    Client::new(peer(), config()).unwrap()
}
fn server(fragment: usize) -> Server {
    Server::new(
        ProtocolId::Standard(gas::ANQP_PROTOCOL_ID),
        server_config(fragment),
    )
    .unwrap()
}
fn wire(body: Body<'_>, token: u8) -> std::vec::Vec<u8> {
    let frame = Frame {
        category: Category::Public,
        dialog_token: token,
        body,
    };
    let mut bytes = std::vec![0; frame.encoded_len().unwrap()];
    frame.encode(&mut bytes).unwrap();
    bytes
}
fn start(
    client: &mut Client,
    server: &mut Server,
) -> (DialogId, DialogId, TxId, std::vec::Vec<u8>) {
    let id = client
        .request(
            Category::Public,
            ProtocolId::Standard(gas::ANQP_PROTOCOL_ID),
            b"query",
            now(0),
        )
        .unwrap();
    let tx = client.transmission().unwrap();
    let bytes = tx.body.to_vec();
    let tx_id = tx.id;
    client.admitted(tx_id, now(0)).unwrap();
    let Event::Query { id: remote_id } = server
        .receive(remote(), SequenceNumber::ZERO, false, &bytes, now(0))
        .unwrap()
    else {
        panic!()
    };
    (id, remote_id, tx_id, bytes)
}
fn send_reply(
    client: &mut Client,
    server: &mut Server,
    at: u64,
    ack: bool,
) -> (TxId, std::vec::Vec<u8>, Progress) {
    let tx = server.transmissions().next().unwrap();
    let bytes = tx.body.to_vec();
    let id = tx.id;
    server.admitted(id, now(at)).unwrap();
    let progress = client.receive(peer(), &bytes, now(at)).unwrap();
    if ack {
        server
            .tx_completed(id, TxOutcome::Acknowledged, now(at))
            .unwrap();
    }
    (id, bytes, progress)
}
fn send_comeback(
    client: &mut Client,
    server: &mut Server,
    sequence: u16,
    at: u64,
) -> (TxId, std::vec::Vec<u8>, Event) {
    let tx = client.transmission().unwrap();
    let bytes = tx.body.to_vec();
    let id = tx.id;
    client.admitted(id, now(at)).unwrap();
    let event = server
        .receive(
            remote(),
            SequenceNumber::new(sequence).unwrap(),
            false,
            &bytes,
            now(at),
        )
        .unwrap();
    (id, bytes, event)
}

#[test]
fn initial_completion_is_once_and_late_submissions_do_not_reopen() {
    let mut client = client();
    let mut server = server(32);
    let (id, remote_id, initial_tx, _) = start(&mut client, &mut server);
    server.respond(remote_id, b"answer", now(0)).unwrap();
    let (tx, bytes, progress) = send_reply(&mut client, &mut server, 0, false);
    assert_eq!(progress, Progress::Finished);
    assert_eq!(client.response(), Some(&b"answer"[..]));
    assert_eq!(
        client.tx_completed(initial_tx, TxOutcome::Failed, now(1)),
        Ok(Progress::Ignored)
    );
    assert_eq!(
        client.receive(peer(), &bytes, now(1)),
        Ok(Progress::Ignored)
    );
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
    assert_eq!(client.take_outcome(), None);
    assert_eq!(
        server.tx_completed(tx, TxOutcome::Acknowledged, now(1)),
        Ok(Event::Delivered { id: remote_id })
    );
    assert_eq!(
        server.tx_completed(tx, TxOutcome::Acknowledged, now(2)),
        Ok(Event::Ignored)
    );
}
#[test]
fn fragments_wait_for_delay_and_fresh_requests_can_precede_old_ack() {
    let mut client = client();
    let mut server = server(3);
    let (id, remote_id, _, _) = start(&mut client, &mut server);
    server.respond(remote_id, b"abcdefgh", now(0)).unwrap();
    let (initial_tx, initial, _) = send_reply(&mut client, &mut server, 0, false);
    assert_eq!(client.response(), None);
    assert_eq!(client.next_deadline(), Some(now(1024)));
    assert_eq!(client.poll(now(1023)), Ok(Progress::Ignored));
    assert!(client.transmission().is_none());
    client.poll(now(1024)).unwrap();
    send_comeback(&mut client, &mut server, 1, 1024);
    assert_eq!(
        server.tx_completed(initial_tx, TxOutcome::Failed, now(1024)),
        Ok(Event::Ignored)
    );
    let (fragment_tx, fragment, progress) = send_reply(&mut client, &mut server, 1024, false);
    assert_eq!(progress, Progress::FragmentAccepted);
    assert_eq!(
        client.receive(peer(), &initial, now(1024)),
        Ok(Progress::Ignored)
    );
    assert_eq!(
        client.receive(peer(), &fragment, now(1024)),
        Ok(Progress::Ignored)
    );
    send_comeback(&mut client, &mut server, 2, 1024);
    assert_eq!(
        server.tx_completed(fragment_tx, TxOutcome::Acknowledged, now(1024)),
        Ok(Event::Ignored)
    );
    send_reply(&mut client, &mut server, 1024, true);
    send_comeback(&mut client, &mut server, 3, 1024);
    assert_eq!(
        send_reply(&mut client, &mut server, 1024, true).2,
        Progress::Finished
    );
    assert_eq!(client.response(), Some(&b"abcdefgh"[..]));
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
}
#[test]
fn mac_retry_replays_fragment_and_initial_replay_cannot_advance_it() {
    let mut client = client();
    let mut server = server(3);
    let (_, remote_id, _, initial_request) = start(&mut client, &mut server);
    server.respond(remote_id, b"abcdefghi", now(0)).unwrap();
    send_reply(&mut client, &mut server, 0, true);
    client.poll(now(1024)).unwrap();
    let (_, comeback, _) = send_comeback(&mut client, &mut server, 1, 1024);
    let tx = server.transmissions().next().unwrap();
    let first = tx.body.to_vec();
    let tx_id = tx.id;
    server.admitted(tx_id, now(1024)).unwrap();
    server
        .tx_completed(tx_id, TxOutcome::Failed, now(1024))
        .unwrap();
    server
        .receive(
            remote(),
            SequenceNumber::new(2).unwrap(),
            false,
            &initial_request,
            now(1024),
        )
        .unwrap();
    let replay = server.transmissions().next().unwrap().id;
    server.admitted(replay, now(1024)).unwrap();
    server
        .tx_completed(replay, TxOutcome::Acknowledged, now(1024))
        .unwrap();
    assert!(matches!(
        server.receive(
            remote(),
            SequenceNumber::new(1).unwrap(),
            true,
            &comeback,
            now(1024)
        ),
        Ok(Event::ReplayQueued { .. })
    ));
    let retry = server.transmissions().next().unwrap();
    assert_eq!(retry.body, first);
    let retry_id = retry.id;
    server.admitted(retry_id, now(1024)).unwrap();
    server
        .tx_completed(retry_id, TxOutcome::Acknowledged, now(1024))
        .unwrap();
    server
        .receive(
            remote(),
            SequenceNumber::new(3).unwrap(),
            false,
            &comeback,
            now(1024),
        )
        .unwrap();
    let Body::ComebackResponse {
        fragment, response, ..
    } = Frame::parse(server.transmissions().next().unwrap().body)
        .unwrap()
        .body
    else {
        panic!()
    };
    assert_eq!(fragment.id(), 1);
    assert_eq!(response, b"def");
}
#[test]
fn repeated_provider_delay_is_a_new_answer_but_does_not_extend_global_timeout() {
    let mut client = client();
    let mut server = server(3);
    let (_, remote_id, _, _) = start(&mut client, &mut server);
    server.defer(remote_id, now(0)).unwrap();
    send_reply(&mut client, &mut server, 0, true);
    for sequence in 1..=3 {
        let at = 1024 * u64::from(sequence);
        client.poll(now(at)).unwrap();
        send_comeback(&mut client, &mut server, sequence, at);
        assert_eq!(
            send_reply(&mut client, &mut server, at, true).2,
            Progress::Waiting
        );
        assert_eq!(client.next_deadline(), Some(now(at + 1024)));
    }
    server.respond(remote_id, b"done", now(3072)).unwrap();
    client.poll(now(4096)).unwrap();
    send_comeback(&mut client, &mut server, 4, 4096);
    send_reply(&mut client, &mut server, 4096, true);
    send_comeback(&mut client, &mut server, 5, 4096);
    send_reply(&mut client, &mut server, 4096, true);
    assert_eq!(client.response(), Some(&b"done"[..]));
}
#[test]
fn lost_comeback_restarts_whole_query_with_fresh_token_and_discards_old_response() {
    let mut client = client();
    let mut server = server(3);
    let (id, remote_id, _, _) = start(&mut client, &mut server);
    server.respond(remote_id, b"abcdef", now(0)).unwrap();
    send_reply(&mut client, &mut server, 0, true);
    client.poll(now(1024)).unwrap();
    send_comeback(&mut client, &mut server, 1, 1024);
    let lost = server.transmissions().next().unwrap().body.to_vec();
    assert_eq!(client.poll(now(4024)), Ok(Progress::Restarted));
    let retry = client.transmission().unwrap();
    let retry_id = retry.id;
    let body = retry.body.to_vec();
    assert_ne!(
        Frame::parse(&body).unwrap().dialog_token,
        Frame::parse(&lost).unwrap().dialog_token
    );
    assert!(matches!(
        Frame::parse(&body).unwrap().body,
        Body::InitialRequest { .. }
    ));
    client.admitted(retry_id, now(4024)).unwrap();
    assert_eq!(
        client.receive(peer(), &lost, now(4024)),
        Ok(Progress::Ignored)
    );
    let Event::Query { id: next } = server
        .receive(
            remote(),
            SequenceNumber::new(2).unwrap(),
            false,
            &body,
            now(4024),
        )
        .unwrap()
    else {
        panic!()
    };
    server.respond(next, b"ok", now(4024)).unwrap();
    let reply = server
        .transmissions()
        .find(|tx| tx.id.dialog == next)
        .unwrap();
    let bytes = reply.body.to_vec();
    client.receive(peer(), &bytes, now(4024)).unwrap();
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
    assert_eq!(client.response(), Some(&b"ok"[..]));
}
#[test]
fn provider_capacity_conflicts_and_expiry_are_explicit() {
    let mut client = client();
    let mut server = server(3);
    let (_, remote_id, _, _) = start(&mut client, &mut server);
    assert_eq!(
        server.respond(remote_id, &[0; 513], now(0)),
        Ok(Event::ResponseQueued { id: remote_id })
    ); // 171 fragments => wire refusal
    let Body::InitialResponse {
        status, response, ..
    } = Frame::parse(server.transmissions().next().unwrap().body)
        .unwrap()
        .body
    else {
        panic!()
    };
    assert_eq!(status, Status::RESPONSE_TOO_LARGE);
    assert!(response.is_empty());
    let tx_id = server.transmissions().next().unwrap().id;
    server.admitted(tx_id, now(0)).unwrap();
    assert_eq!(
        server.tx_completed(tx_id, TxOutcome::Acknowledged, now(40000)),
        Ok(Event::TimedOut { id: remote_id })
    );
    assert_eq!(server.poll(now(40000)), Ok(Event::Ignored));
    assert_eq!(
        server.respond(remote_id, &[], now(40000)),
        Err(Error::WrongOperation)
    );
}
#[test]
fn invalid_rx_preserves_live_dialog_and_correlation() {
    let mut client = client();
    let id = client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
        .unwrap();
    let tx_id = client.transmission().unwrap().id;
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(127, false).unwrap(),
        protocol: ProtocolId::Standard(0),
    };
    let reply = wire(
        Body::InitialResponse {
            status: Status::SUCCESS,
            comeback_delay_tu: 0,
            advertisement,
            response: b"ok",
        },
        0,
    );
    assert_eq!(
        client.receive(peer(), &reply, now(0)),
        Ok(Progress::Ignored)
    );
    client.admitted(tx_id, now(0)).unwrap();
    assert!(client.receive(peer(), &[4, 11], now(0)).is_err());
    assert_eq!(
        client.receive(PeerIdentity { epoch: 2, ..peer() }, &reply, now(0)),
        Ok(Progress::Ignored)
    );
    assert_eq!(
        client.receive(peer(), &reply, now(0)),
        Ok(Progress::Finished)
    );
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
}
#[test]
fn tokens_are_quarantined_until_explicit_guard_expires_and_time_is_checked() {
    let mut client = client();
    for token in 0..=u8::MAX {
        let id = client
            .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
            .unwrap();
        assert_eq!(
            Frame::parse(client.transmission().unwrap().body)
                .unwrap()
                .dialog_token,
            token
        );
        client.cancel(id).unwrap();
        assert_eq!(client.take_outcome(), Some(Outcome::Cancelled { id }));
    }
    assert_eq!(
        client.request(Category::Public, ProtocolId::Standard(0), &[], now(0)),
        Err(Error::TokensExhausted)
    );
    assert_eq!(client.next_deadline(), Some(now(60000)));
    assert_eq!(
        client.request(Category::Public, ProtocolId::Standard(0), &[], now(59999)),
        Err(Error::TokensExhausted)
    );
    client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(60000))
        .unwrap();
    assert_eq!(
        Frame::parse(client.transmission().unwrap().body)
            .unwrap()
            .dialog_token,
        0
    );
    let mut fresh = Client::new(peer(), config()).unwrap();
    assert_eq!(
        fresh.request(
            Category::Public,
            ProtocolId::Standard(0),
            &[],
            now(u64::MAX)
        ),
        Err(Error::TimeOverflow)
    );
    assert_eq!(fresh.poll(now(1)), Err(Error::TimeWentBackwards));
}
#[test]
fn bad_fragment_and_small_assembly_fail_without_accepting_partial_result() {
    let mut client = requester::Requester::<128, 2>::new(peer(), config()).unwrap();
    client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(0)).unwrap();
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(127, false).unwrap(),
        protocol: ProtocolId::Standard(0),
    };
    let reply = wire(
        Body::InitialResponse {
            status: Status::SUCCESS,
            comeback_delay_tu: 1,
            advertisement,
            response: &[],
        },
        0,
    );
    client.receive(peer(), &reply, now(0)).unwrap();
    client.poll(now(1024)).unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(1024)).unwrap();
    let wrong = wire(
        Body::ComebackResponse {
            status: Status::SUCCESS,
            fragment: Fragment::new(1, false).unwrap(),
            comeback_delay_tu: 0,
            advertisement,
            response: b"a",
        },
        0,
    );
    assert_eq!(
        client.receive(peer(), &wrong, now(1024)),
        Err(Error::UnexpectedFragment {
            expected: 0,
            received: 1
        })
    );
    let large = wire(
        Body::ComebackResponse {
            status: Status::SUCCESS,
            fragment: Fragment::new(0, false).unwrap(),
            comeback_delay_tu: 0,
            advertisement,
            response: b"abc",
        },
        0,
    );
    assert_eq!(
        client.receive(peer(), &large, now(1024)),
        Err(Error::Capacity {
            required: 3,
            capacity: 2
        })
    );
    assert_eq!(client.response(), None);
    assert_eq!(client.take_outcome(), None);
}

#[test]
fn complete_128_fragment_sequence_and_sequence_reuse_without_retry() {
    let mut client = client();
    let mut server = server(1);
    let (id, remote_id, _, _) = start(&mut client, &mut server);
    let payload: std::vec::Vec<u8> = (0..128).collect();
    server.respond(remote_id, &payload, now(0)).unwrap();
    send_reply(&mut client, &mut server, 0, true);
    client.poll(now(1024)).unwrap();
    for fragment in 0..128 {
        // Reusing a sequence with Retry clear is a genuinely new request;
        // real management sequence spaces may wrap while a dialog is live.
        send_comeback(&mut client, &mut server, 1, 1024);
        let reply = server.transmissions().next().unwrap();
        let Body::ComebackResponse {
            fragment: found, ..
        } = Frame::parse(reply.body).unwrap().body
        else {
            panic!()
        };
        assert_eq!(usize::from(found.id()), fragment);
        assert_eq!(found.more(), fragment != 127);
        send_reply(&mut client, &mut server, 1024, true);
    }
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
    assert_eq!(client.response(), Some(payload.as_slice()));
}
#[test]
fn responder_capacity_is_atomic_and_advertised_limit_refuses_oversize() {
    let mut client = client();
    let mut server = server(32);
    let (_, id, _, _) = start(&mut client, &mut server);
    assert_eq!(
        server.respond(id, &[0; 513], now(0)),
        Err(Error::Capacity {
            required: 513,
            capacity: 512
        })
    );
    assert!(server.transmissions().next().is_none());
    assert_eq!(server.query(id), Some(&b"query"[..]));
    server.respond(id, b"ok", now(0)).unwrap();
    let mut limited_config = server_config(32);
    limited_config.response_info = ResponseInfo::new(1, false).unwrap();
    let mut limited = Server::new(ProtocolId::Standard(0), limited_config).unwrap();
    let mut next_client = Client::new(peer(), config()).unwrap();
    let (_, id, _, _) = start(&mut next_client, &mut limited);
    limited.respond(id, &[0; 257], now(0)).unwrap();
    let Body::InitialResponse { status, .. } =
        Frame::parse(limited.transmissions().next().unwrap().body)
            .unwrap()
            .body
    else {
        panic!()
    };
    assert_eq!(status, Status::RESPONSE_TOO_LARGE);
}
#[test]
fn slots_and_replay_cache_are_not_evicted_and_cancel_invalidates_provider() {
    let mut server = server(32);
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(0, false).unwrap(),
        protocol: ProtocolId::Standard(0),
    };
    let first = wire(
        Body::InitialRequest {
            advertisement,
            query: b"a",
        },
        1,
    );
    let Event::Query { id: first_id } = server
        .receive(remote(), SequenceNumber::ZERO, false, &first, now(0))
        .unwrap()
    else {
        panic!()
    };
    let second = wire(
        Body::InitialRequest {
            advertisement,
            query: b"b",
        },
        2,
    );
    let Event::Query { id: second_id } = server
        .receive(
            remote(),
            SequenceNumber::ZERO.next(),
            false,
            &second,
            now(0),
        )
        .unwrap()
    else {
        panic!()
    };
    server.respond(first_id, b"done", now(0)).unwrap();
    let tx = server.transmissions().next().unwrap().id;
    assert_eq!(
        server.tx_completed(tx, TxOutcome::Acknowledged, now(0)),
        Err(Error::NotAdmitted)
    );
    server.admitted(tx, now(0)).unwrap();
    assert_eq!(server.admitted(tx, now(0)), Err(Error::AlreadyAdmitted));
    server
        .tx_completed(tx, TxOutcome::Acknowledged, now(0))
        .unwrap();
    let third = wire(
        Body::InitialRequest {
            advertisement,
            query: b"c",
        },
        3,
    );
    assert_eq!(
        server.receive(
            remote(),
            SequenceNumber::new(2).unwrap(),
            false,
            &third,
            now(0)
        ),
        Err(Error::Capacity {
            required: 3,
            capacity: 2
        })
    );
    server.cancel(second_id).unwrap();
    assert_eq!(
        server.respond(second_id, b"late", now(0)),
        Err(Error::WrongOperation)
    );
    assert!(matches!(
        server.receive(
            remote(),
            SequenceNumber::new(2).unwrap(),
            false,
            &third,
            now(0)
        ),
        Ok(Event::Query { .. })
    ));
    assert_eq!(
        server.poll(now(40000)),
        Ok(Event::CacheExpired { id: first_id })
    );
    assert!(matches!(
        server.poll(now(40000)),
        Ok(Event::TimedOut { .. })
    ));
    assert_eq!(server.poll(now(40000)), Ok(Event::Ignored));
}
#[test]
fn oversized_unsupported_advertisement_does_not_reserve_an_invisible_slot() {
    let mut server = server(32);
    let vendor = [7; 200];
    let bad = wire(
        Body::InitialRequest {
            advertisement: AdvertisementProtocol {
                info: ResponseInfo::new(0, false).unwrap(),
                protocol: ProtocolId::Vendor(&vendor),
            },
            query: &[],
        },
        0,
    );
    assert!(matches!(
        server.receive(remote(), SequenceNumber::ZERO, false, &bad, now(0)),
        Err(Error::Capacity { .. })
    ));
    let mut client = client();
    start(&mut client, &mut server);
    let mut other = Client::new(PeerIdentity { epoch: 2, ..peer() }, config()).unwrap();
    other
        .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
        .unwrap();
    let bytes = other.transmission().unwrap().body.to_vec();
    assert!(matches!(
        server.receive(
            PeerIdentity {
                epoch: 2,
                ..remote()
            },
            SequenceNumber::ZERO,
            false,
            &bytes,
            now(0)
        ),
        Ok(Event::Query { .. })
    ));
}
#[test]
fn deferred_provider_rejection_and_unknown_comeback_have_terminal_statuses() {
    let mut client = client();
    let mut server = server(32);
    let (id, remote_id, _, _) = start(&mut client, &mut server);
    server.defer(remote_id, now(0)).unwrap();
    send_reply(&mut client, &mut server, 0, true);
    assert_eq!(
        server.reject(remote_id, Status::SERVER_UNREACHABLE, now(0)),
        Ok(Event::ProviderRejected { id: remote_id })
    );
    client.poll(now(1024)).unwrap();
    send_comeback(&mut client, &mut server, 1, 1024);
    send_reply(&mut client, &mut server, 1024, true);
    assert_eq!(
        client.take_outcome(),
        Some(Outcome::Rejected {
            id,
            status: Status::SERVER_UNREACHABLE
        })
    );
    let unknown = wire(Body::ComebackRequest, 123);
    server
        .receive(
            remote(),
            SequenceNumber::new(3).unwrap(),
            false,
            &unknown,
            now(1024),
        )
        .unwrap();
    let Body::ComebackResponse { status, .. } =
        Frame::parse(server.transmissions().next().unwrap().body)
            .unwrap()
            .body
    else {
        panic!()
    };
    assert_eq!(status, Status::NO_OUTSTANDING_REQUEST);
}
#[test]
fn completion_at_expiry_times_out_once_instead_of_reviving_requester() {
    let mut client = client();
    let id = client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(0)).unwrap();
    assert_eq!(
        client.tx_completed(tx, TxOutcome::Acknowledged, now(20000)),
        Ok(Progress::Finished)
    );
    assert_eq!(client.take_outcome(), Some(Outcome::TimedOut { id }));
    assert_eq!(client.poll(now(20000)), Ok(Progress::Ignored));
    assert!(client.response().is_none());
}

#[test]
fn outstanding_status_can_carry_fragments_and_terminal_error_payload_is_never_a_result() {
    let mut client = client();
    let id = client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(0)).unwrap();
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(127, false).unwrap(),
        protocol: ProtocolId::Standard(0),
    };
    client
        .receive(
            peer(),
            &wire(
                Body::InitialResponse {
                    status: Status::SUCCESS,
                    comeback_delay_tu: 1,
                    advertisement,
                    response: &[],
                },
                0,
            ),
            now(0),
        )
        .unwrap();
    client.poll(now(1024)).unwrap();
    for fragment in 0..2 {
        let tx = client.transmission().unwrap().id;
        client.admitted(tx, now(1024)).unwrap();
        let reply = wire(
            Body::ComebackResponse {
                status: Status::RESPONSE_OUTSTANDING,
                fragment: Fragment::new(fragment, fragment == 0).unwrap(),
                comeback_delay_tu: 0,
                advertisement,
                response: &[fragment],
            },
            0,
        );
        client.receive(peer(), &reply, now(1024)).unwrap();
    }
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
    assert_eq!(client.response(), Some(&[0, 1][..]));
    let id = client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(1024))
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(1024)).unwrap();
    let unknown = Status(777);
    client
        .receive(
            peer(),
            &wire(
                Body::InitialResponse {
                    status: unknown,
                    comeback_delay_tu: 0,
                    advertisement,
                    response: b"not an answer",
                },
                1,
            ),
            now(1024),
        )
        .unwrap();
    assert_eq!(
        client.take_outcome(),
        Some(Outcome::Rejected {
            id,
            status: unknown
        })
    );
    assert!(client.response().is_none());
}

#[test]
fn restart_token_exhaustion_waits_without_spinning_or_extending_deadline() {
    let mut client = client();
    for _ in 0..u8::MAX {
        let id = client
            .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
            .unwrap();
        client.cancel(id).unwrap();
        client.take_outcome();
    }
    let id = client
        .request(Category::Public, ProtocolId::Standard(0), &[], now(0))
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(0)).unwrap();
    assert_eq!(client.poll(now(3000)), Ok(Progress::Waiting));
    assert_eq!(client.next_deadline(), Some(now(20000)));
    assert!(client.transmission().is_none());
    assert_eq!(client.poll(now(20000)), Ok(Progress::Finished));
    assert_eq!(client.take_outcome(), Some(Outcome::TimedOut { id }));
    assert_eq!(client.next_deadline(), Some(now(60000)));
}

#[test]
fn older_mac_retry_cannot_commit_newer_fragment_and_ambiguous_order_is_an_error() {
    let mut client = client();
    let mut server = server(3);
    let (_, id, _, _) = start(&mut client, &mut server);
    server.respond(id, b"abcdef", now(0)).unwrap();
    send_reply(&mut client, &mut server, 0, true);
    client.poll(now(1024)).unwrap();
    let (_, old_request, _) = send_comeback(&mut client, &mut server, 1, 1024);
    send_reply(&mut client, &mut server, 1024, true);
    let (_, new_request, _) = send_comeback(&mut client, &mut server, 2, 1024);
    let tx = server.transmissions().next().unwrap().id;
    server.admitted(tx, now(1024)).unwrap();
    assert_eq!(
        server.receive(
            remote(),
            SequenceNumber::new(1).unwrap(),
            true,
            &old_request,
            now(1024)
        ),
        Ok(Event::Ignored)
    );
    let ambiguous = SequenceNumber::new(2)
        .unwrap()
        .wrapping_add(SequenceNumber::HALF_SPACE);
    assert_eq!(
        server.receive(remote(), ambiguous, true, &new_request, now(1024)),
        Err(Error::AmbiguousSequence)
    );
    assert_eq!(
        server.tx_completed(tx, TxOutcome::Failed, now(1024)),
        Ok(Event::DeliveryFailed { id, tx })
    );
    assert_eq!(
        server.receive(
            remote(),
            SequenceNumber::new(2).unwrap(),
            true,
            &new_request,
            now(1024)
        ),
        Ok(Event::ReplayQueued { id })
    );
    assert_eq!(
        send_reply(&mut client, &mut server, 1024, true).2,
        Progress::Finished
    );
    assert_eq!(client.response(), Some(&b"abcdef"[..]));
}
#[test]
fn no_outstanding_rejection_does_not_require_unknown_dialog_protocol_echo() {
    let mut client = client();
    let id = client
        .request(Category::Public, ProtocolId::Standard(1), &[], now(0))
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(0)).unwrap();
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(127, false).unwrap(),
        protocol: ProtocolId::Standard(1),
    };
    client
        .receive(
            peer(),
            &wire(
                Body::InitialResponse {
                    status: Status::SUCCESS,
                    comeback_delay_tu: 1,
                    advertisement,
                    response: &[],
                },
                0,
            ),
            now(0),
        )
        .unwrap();
    client.poll(now(1024)).unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, now(1024)).unwrap();
    let advertisement = AdvertisementProtocol {
        protocol: ProtocolId::Standard(0),
        ..advertisement
    };
    let reply = wire(
        Body::ComebackResponse {
            status: Status::NO_OUTSTANDING_REQUEST,
            fragment: Fragment::new(0, false).unwrap(),
            comeback_delay_tu: 0,
            advertisement,
            response: &[],
        },
        0,
    );
    assert_eq!(
        client.receive(peer(), &reply, now(1024)),
        Ok(Progress::Finished)
    );
    assert_eq!(
        client.take_outcome(),
        Some(Outcome::Rejected {
            id,
            status: Status::NO_OUTSTANDING_REQUEST
        })
    );
    assert!(client.response().is_none());
}
