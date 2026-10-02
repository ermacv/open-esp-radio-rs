use super::*;

#[test]
fn home_realm_filters_names_without_changing_eap_and_full_query_wins() {
    let full = [1, 0, 6, 0, 0, 3, b'a', b';', b'b', 0];
    let mut home = [0u8; 32];
    let home_length = hs20::HomeRealms::encode(
        &[hs20::HomeRealm {
            encoding: 0,
            realms: b"b;x;b",
        }],
        &mut home,
    )
    .unwrap();
    let mut query_bytes = [0u8; 64];
    let home_query = hs20::Element {
        subtype: hs20::Subtype::NAI_HOME_REALM_QUERY,
        reserved: 0,
        body: &home[..home_length],
    };
    let length = home_query.encode(&mut query_bytes).unwrap();
    let query = Query::parse(&query_bytes[..length]).unwrap();
    let response = Response::<64>::prepare(
        query,
        &[Resolution::Record(Element {
            id: InfoId::NAI_REALM,
            body: &full,
        })],
    )
    .unwrap();
    let Value::NaiRealms(filtered) = response.elements().iter().next().unwrap().value().unwrap()
    else {
        panic!()
    };
    assert_eq!(filtered.count(), 1);
    assert_eq!(filtered.iter().next().unwrap().realms, b"b");
    validate_response(query, response.as_bytes()).unwrap();
    let mut unfiltered = [0u8; 64];
    let raw_length = Element {
        id: InfoId::NAI_REALM,
        body: &full,
    }
    .encode(&mut unfiltered)
    .unwrap();
    assert_eq!(
        validate_response(query, &unfiltered[..raw_length]),
        Err(Error::UnrequestedAnswer)
    );
    let end =
        wire::QueryList::encode(&[InfoId::NAI_REALM], &mut query_bytes[length..]).unwrap() + length;
    let query = Query::parse(&query_bytes[..end]).unwrap();
    assert_eq!(query.requests().count(), 1);
    let response = Response::<64>::prepare(
        query,
        &[Resolution::Record(Element {
            id: InfoId::NAI_REALM,
            body: &full,
        })],
    )
    .unwrap();
    assert_eq!(response.elements().iter().next().unwrap().body, full);
}
#[test]
fn wrong_missing_duplicate_and_unsolicited_answers_are_explicit() {
    let mut bytes = [0u8; 32];
    let length = wire::QueryList::encode(&[InfoId::DOMAIN_NAME], &mut bytes).unwrap();
    let query = Query::parse(&bytes[..length]).unwrap();
    assert!(matches!(
        Response::<32>::prepare(query, &[]),
        Err(Error::AnswerCount { .. })
    ));
    assert!(matches!(
        Response::<32>::prepare(
            query,
            &[Resolution::Record(Element {
                id: InfoId::IP_ADDRESS_AVAILABILITY,
                body: &[0]
            })]
        ),
        Err(Error::AnswerIdentity)
    ));
    assert!(
        Response::<32>::prepare(query, &[Resolution::Unsupported])
            .unwrap()
            .as_bytes()
            .is_empty()
    );
    let mut unsolicited = [0u8; 32];
    let length = Element {
        id: InfoId::IP_ADDRESS_AVAILABILITY,
        body: &[0],
    }
    .encode(&mut unsolicited)
    .unwrap();
    assert_eq!(
        validate_response(query, &unsolicited[..length]),
        Err(Error::UnrequestedAnswer)
    );
    assert_eq!(Query::parse(&[]), Err(Error::EmptyQuery));
}

#[test]
fn icon_query_maps_to_binary_file_and_osu_nai_count_must_match() {
    let mut query_bytes = [0u8; 128];
    let length = hs20::Element {
        subtype: hs20::Subtype::ICON_REQUEST,
        reserved: 0,
        body: b"logo.png",
    }
    .encode(&mut query_bytes)
    .unwrap();
    let query = Query::parse(&query_bytes[..length]).unwrap();
    assert_eq!(query.requests().next(), Some(Request::Icon("logo.png")));
    let mut file = [0u8; 32];
    let file_length = hs20::IconBinaryFile {
        status: hs20::IconStatus::SUCCESS,
        mime: "image/png",
        data: b"image",
    }
    .encode(&mut file)
    .unwrap();
    let mut response_bytes = [0u8; 128];
    let length = hs20::Element {
        subtype: hs20::Subtype::ICON_BINARY_FILE,
        reserved: 0,
        body: &file[..file_length],
    }
    .encode(&mut response_bytes)
    .unwrap();
    let record = Elements::parse(&response_bytes[..length])
        .unwrap()
        .iter()
        .next()
        .unwrap();
    let response = Response::<128>::prepare(query, &[Resolution::Record(record)]).unwrap();
    validate_response(query, response.as_bytes()).unwrap();
    let length = hs20::Element {
        subtype: hs20::Subtype::QUERY_LIST,
        reserved: 0,
        body: &[
            hs20::Subtype::OSU_PROVIDERS.0,
            hs20::Subtype::OSU_PROVIDERS_NAI.0,
        ],
    }
    .encode(&mut query_bytes)
    .unwrap();
    let query = Query::parse(&query_bytes[..length]).unwrap();
    let length = hs20::Element {
        subtype: hs20::Subtype::OSU_PROVIDERS,
        reserved: 0,
        body: &[0, 0],
    }
    .encode(&mut response_bytes)
    .unwrap();
    let extra = hs20::Element {
        subtype: hs20::Subtype::OSU_PROVIDERS_NAI,
        reserved: 0,
        body: &[1, b'a'],
    }
    .encode(&mut response_bytes[length..])
    .unwrap();
    let elements = Elements::parse(&response_bytes[..length + extra]).unwrap();
    let resolutions = elements
        .iter()
        .map(Resolution::Record)
        .collect::<std::vec::Vec<_>>();
    assert!(matches!(
        Response::<128>::prepare(query, &resolutions),
        Err(Error::ProviderNaiCount)
    ));
    assert_eq!(
        validate_response(query, elements.as_bytes()),
        Err(Error::ProviderNaiCount)
    );
}
#[test]
fn requester_never_reports_transport_success_for_invalid_anqp() {
    use crate::{Category, PeerIdentity, requester};
    use oer_ieee80211_mac::gas::{
        self, AdvertisementProtocol, Body, Frame, ProtocolId, ResponseInfo, Status,
    };
    use oer_time::{Duration, Instant};
    let peer = PeerIdentity {
        local: [2, 0, 0, 0, 0, 1],
        peer: [2, 0, 0, 0, 0, 2],
        bssid: [2, 0, 0, 0, 0, 2],
        epoch: 1,
    };
    let mut client = Requester::<128, 128>::new(
        peer,
        requester::Config {
            dialog_timeout: Duration::from_millis(100),
            response_timeout: Duration::from_millis(10),
            max_restarts: 0,
            token_reuse_guard: Duration::from_secs(1),
        },
    )
    .unwrap();
    let query_bytes = [0, 1, 2, 0, 12, 1];
    let id = client
        .request(
            Category::Public,
            Query::parse(&query_bytes).unwrap(),
            Instant::EPOCH,
        )
        .unwrap();
    let tx = client.transmission().unwrap().id;
    client.admitted(tx, Instant::EPOCH).unwrap();
    let advertisement = AdvertisementProtocol {
        info: ResponseInfo::new(127, false).unwrap(),
        protocol: ProtocolId::Standard(gas::ANQP_PROTOCOL_ID),
    };
    let frame = Frame {
        category: Category::Public,
        dialog_token: 0,
        body: Body::InitialResponse {
            status: Status::SUCCESS,
            comeback_delay_tu: 0,
            advertisement,
            response: &[1, 1, 1],
        },
    };
    let mut bytes = [0u8; 128];
    let length = frame.encode(&mut bytes).unwrap();
    client
        .receive(peer, &bytes[..length], Instant::EPOCH)
        .unwrap();
    assert!(
        matches!(client.take_outcome(), Some(Outcome::InvalidResponse { id: bad, .. }) if bad == id)
    );
    assert!(client.response().is_err());
    assert_eq!(client.take_outcome(), None);
}

#[test]
fn anqp_client_and_provider_exchange_complete_records_across_gas_fragments() {
    use crate::{Category, PeerIdentity, TxOutcome, requester, responder};
    use oer_ieee80211_mac::{
        gas::{self, ProtocolId, ResponseInfo},
        management::IEEE_TIME_UNIT_MICROS,
        sequence::SequenceNumber,
    };
    use oer_time::{Duration, Instant};
    let peer = PeerIdentity {
        local: [2, 0, 0, 0, 0, 1],
        peer: [2, 0, 0, 0, 0, 2],
        bssid: [2, 0, 0, 0, 0, 2],
        epoch: 1,
    };
    let remote = PeerIdentity {
        local: peer.peer,
        peer: peer.local,
        ..peer
    };
    let config = requester::Config {
        dialog_timeout: Duration::from_secs(1),
        response_timeout: Duration::from_millis(50),
        max_restarts: 1,
        token_reuse_guard: Duration::from_secs(1),
    };
    let mut client = Requester::<128, 512>::new(peer, config).unwrap();
    let mut server = responder::Responder::<2, 64, 512, 128>::new(
        ProtocolId::Standard(gas::ANQP_PROTOCOL_ID),
        responder::Config {
            dialog_timeout: Duration::from_secs(1),
            fragment_payload_limit: 3,
            comeback_delay_tu: 1,
            response_info: ResponseInfo::new(127, false).unwrap(),
        },
    )
    .unwrap();
    let mut query_bytes = [0u8; 32];
    let length = wire::QueryList::encode(&[InfoId::DOMAIN_NAME], &mut query_bytes).unwrap();
    let id = client
        .request(
            Category::Public,
            Query::parse(&query_bytes[..length]).unwrap(),
            Instant::EPOCH,
        )
        .unwrap();
    let tx = client.transmission().unwrap();
    let bytes = tx.body.to_vec();
    let tx_id = tx.id;
    client.admitted(tx_id, Instant::EPOCH).unwrap();
    let responder::Event::Query { id: server_id } = server
        .receive(remote, SequenceNumber::ZERO, false, &bytes, Instant::EPOCH)
        .unwrap()
    else {
        panic!()
    };
    let query = Query::parse(server.query(server_id).unwrap()).unwrap();
    assert_eq!(
        query.requests().next(),
        Some(Request::Standard(InfoId::DOMAIN_NAME))
    );
    let record = Element {
        id: InfoId::DOMAIN_NAME,
        body: b"\x07example",
    };
    let response = Response::<128>::prepare(query, &[Resolution::Record(record)]).unwrap();
    server
        .respond(server_id, response.as_bytes(), Instant::EPOCH)
        .unwrap();
    let tx = server.transmissions().next().unwrap();
    let bytes = tx.body.to_vec();
    let tx_id = tx.id;
    server.admitted(tx_id, Instant::EPOCH).unwrap();
    client.receive(peer, &bytes, Instant::EPOCH).unwrap();
    server
        .tx_completed(tx_id, TxOutcome::Acknowledged, Instant::EPOCH)
        .unwrap();
    let at = Instant::from_micros(IEEE_TIME_UNIT_MICROS);
    client.poll(at).unwrap();
    for sequence in 1..=4 {
        let tx = client.transmission().unwrap();
        let bytes = tx.body.to_vec();
        let tx_id = tx.id;
        client.admitted(tx_id, at).unwrap();
        server
            .receive(
                remote,
                SequenceNumber::new(sequence).unwrap(),
                false,
                &bytes,
                at,
            )
            .unwrap();
        let tx = server.transmissions().next().unwrap();
        let bytes = tx.body.to_vec();
        let tx_id = tx.id;
        server.admitted(tx_id, at).unwrap();
        client.receive(peer, &bytes, at).unwrap();
        server
            .tx_completed(tx_id, TxOutcome::Acknowledged, at)
            .unwrap();
    }
    assert_eq!(client.take_outcome(), Some(Outcome::Complete { id }));
    assert_eq!(
        client.response().unwrap().unwrap().iter().next(),
        Some(record)
    );
}

#[test]
fn hotspot_duplicates_are_checked_across_the_full_subtype_space() {
    let mut query_bytes = [0u8; 32];
    let length = hs20::Element {
        subtype: hs20::Subtype::QUERY_LIST,
        reserved: 0,
        body: &[u8::MAX],
    }
    .encode(&mut query_bytes)
    .unwrap();
    let query = Query::parse(&query_bytes[..length]).unwrap();
    let mut bytes = [0u8; 32];
    let record = hs20::Element {
        subtype: hs20::Subtype(u8::MAX),
        reserved: 0,
        body: &[],
    };
    let first = record.encode(&mut bytes).unwrap();
    let second = record.encode(&mut bytes[first..]).unwrap();
    assert_eq!(
        validate_response(query, &bytes[..first + second]),
        Err(Error::DuplicateHotspot(hs20::Subtype(u8::MAX)))
    );
}
