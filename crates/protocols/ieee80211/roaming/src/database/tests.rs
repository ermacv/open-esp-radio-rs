use super::*;
use oer_ieee80211_mac::channel::ChannelWidth;

fn link() -> LinkIdentity {
    LinkIdentity {
        peer: [2, 0, 0, 0, 0, 1],
        generation: 7,
    }
}
fn time(us: u64) -> Instant {
    Instant::from_micros(us)
}
fn ttl(us: u64) -> Duration {
    Duration::from_micros(us)
}
fn context() -> ReportContext {
    ReportContext {
        source: ReportSource::NeighborResponse { token: 1 },
        ssid: Some(WifiSsid::new(b"a\0b").unwrap()),
    }
}
fn report(bssid: u8, preference: u8) -> std::vec::Vec<u8> {
    std::vec![
        52, 21, 2, 0, 0, 0, 0, bssid, 0, 0, 0, 0, 81, 6, 7, 3, 1, preference, 221, 3, 1, 2, 3
    ]
}
fn observation(bssid: u8, rssi: i8) -> Observation {
    let mut record = ScanRecord {
        bssid: [2, 0, 0, 0, 0, bssid],
        channel: 6,
        rssi,
        ssid_len: 3,
        ..ScanRecord::EMPTY
    };
    record.ssid[..3].copy_from_slice(b"a\0b");
    Observation::new(
        record,
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
        Elements::parse(&[
            70, 5, 3, 0, 0, 0, 0, 127, 4, 0, 0, 8, 128, 11, 5, 4, 0, 100, 8, 0,
        ])
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn reports_and_observations_are_independent_and_expire_individually() {
    let mut database = NeighborDatabase::<2, 32>::new(link());
    let wire = report(9, 200);
    database
        .update_reports(
            link(),
            Elements::parse(&wire).unwrap(),
            context(),
            time(0),
            ttl(10),
        )
        .unwrap();
    let entry = database.get([2, 0, 0, 0, 0, 9], time(1)).unwrap().unwrap();
    assert!(entry.observation.is_none());
    let report = entry.report.unwrap();
    assert_eq!(report.report.preference().unwrap(), Some(200));
    assert_eq!(
        report.report.subelements.unique(221).unwrap(),
        Some(&[1, 2, 3][..])
    );
    assert_eq!(report.context.ssid.unwrap().as_bytes(), b"a\0b");
    database
        .observe(link(), observation(9, -60), time(1), ttl(20))
        .unwrap();
    assert_eq!(database.next_deadline(), Some(time(10)));
    let entry = database.get([2, 0, 0, 0, 0, 9], time(10)).unwrap().unwrap();
    assert!(entry.report.is_none());
    assert_eq!(entry.observation.unwrap().observation.record().rssi, -60);
    assert_eq!(
        database.expire(time(10)).unwrap(),
        DatabaseEvent::Expired {
            reports: 1,
            observations: 0
        }
    );
    assert_eq!(database.next_deadline(), Some(time(21)));
    assert_eq!(
        database.expire(time(21)).unwrap(),
        DatabaseEvent::Expired {
            reports: 0,
            observations: 1
        }
    );
    assert!(database.entries(time(21)).unwrap().next().is_none());
}

#[test]
fn capability_bits_and_load_are_retained_without_claiming_local_support() {
    let observation = observation(9, -50);
    assert!(observation.capabilities().neighbor_report());
    assert!(observation.capabilities().link_measurement());
    assert!(observation.capabilities().bss_transition());
    assert!(observation.capabilities().extended_bit(31));
    assert_eq!(
        observation.capabilities().radio_measurement(),
        Some(&[3, 0, 0, 0, 0])
    );
    assert_eq!(observation.capabilities().extended(), [0, 0, 8, 128]);
    assert_eq!(
        observation.load(),
        Some(BssLoad {
            station_count: 4,
            channel_utilization: 100,
            available_admission_capacity: 8
        })
    );
}

#[test]
fn entire_report_batch_is_rejected_when_full_malformed_or_duplicated() {
    let mut database = NeighborDatabase::<2, 32>::new(link());
    database
        .observe(link(), observation(9, -60), time(0), ttl(100))
        .unwrap();
    let reports = [report(8, 10), report(7, 20)].concat();
    assert_eq!(
        database.update_reports(
            link(),
            Elements::parse(&reports).unwrap(),
            context(),
            time(1),
            ttl(10)
        ),
        Err(DatabaseError::Full)
    );
    assert!(database.get([2, 0, 0, 0, 0, 8], time(1)).unwrap().is_none());
    let reports = [report(8, 10), report(8, 20)].concat();
    assert!(matches!(
        database.update_reports(
            link(),
            Elements::parse(&reports).unwrap(),
            context(),
            time(1),
            ttl(10)
        ),
        Err(DatabaseError::DuplicateBssid(_))
    ));
    let mut reports = report(8, 10);
    reports.extend_from_slice(&[52, 1, 0]);
    assert!(
        database
            .update_reports(
                link(),
                Elements::parse(&reports).unwrap(),
                context(),
                time(1),
                ttl(10)
            )
            .is_err()
    );
    assert_eq!(database.entries(time(1)).unwrap().count(), 1);
}

#[test]
fn expired_storage_can_be_reused_without_collision_in_prepared_batch() {
    let mut database = NeighborDatabase::<2, 32>::new(link());
    database
        .observe(link(), observation(9, -60), time(0), ttl(1))
        .unwrap();
    let reports = [report(8, 10), report(9, 20)].concat();
    database
        .update_reports(
            link(),
            Elements::parse(&reports).unwrap(),
            context(),
            time(2),
            ttl(10),
        )
        .unwrap();
    assert_eq!(database.entries(time(2)).unwrap().count(), 2);
    assert_eq!(
        database
            .get([2, 0, 0, 0, 0, 8], time(2))
            .unwrap()
            .unwrap()
            .report
            .unwrap()
            .report
            .preference()
            .unwrap(),
        Some(10)
    );
    assert_eq!(
        database
            .get([2, 0, 0, 0, 0, 9], time(2))
            .unwrap()
            .unwrap()
            .report
            .unwrap()
            .report
            .preference()
            .unwrap(),
        Some(20)
    );
}

#[test]
fn stale_epoch_old_time_overflow_and_short_report_capacity_preserve_data() {
    let mut database = NeighborDatabase::<1, 16>::new(link());
    database
        .observe(link(), observation(9, -60), time(10), ttl(20))
        .unwrap();
    assert_eq!(
        database
            .observe(
                LinkIdentity {
                    generation: 6,
                    ..link()
                },
                observation(8, -50),
                time(11),
                ttl(20)
            )
            .unwrap(),
        DatabaseEvent::Ignored
    );
    assert_eq!(
        database.observe(link(), observation(8, -50), time(9), ttl(20)),
        Err(DatabaseError::Protocol(Error::TimeBeforeOperation))
    );
    assert_eq!(
        database.observe(link(), observation(8, -50), time(u64::MAX), ttl(20)),
        Err(DatabaseError::Protocol(Error::TimeOverflow))
    );
    let wire = report(9, 200);
    assert!(matches!(
        database.update_reports(
            link(),
            Elements::parse(&wire).unwrap(),
            context(),
            time(11),
            ttl(20)
        ),
        Err(DatabaseError::Protocol(Error::FrameTooLarge { .. }))
    ));
    assert!(
        database
            .get([2, 0, 0, 0, 0, 9], time(11))
            .unwrap()
            .unwrap()
            .report
            .is_none()
    );
    assert_eq!(database.next_deadline(), Some(time(30)));
}

#[test]
fn forged_scan_lengths_and_duplicate_capability_elements_are_rejected() {
    let channel = Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap();
    let mut record = *observation(9, -60).record();
    record.ssid_len = 33;
    assert_eq!(
        Observation::new(record, channel, Elements::EMPTY),
        Err(DatabaseError::MalformedObservation)
    );
    record.ssid_len = 3;
    record.rsn_ie_len = 255;
    if usize::from(record.rsn_ie_len) > record.rsn_ie.len() {
        assert_eq!(
            Observation::new(record, channel, Elements::EMPTY),
            Err(DatabaseError::MalformedObservation)
        );
    }
    assert!(
        Observation::new(
            *observation(9, -60).record(),
            channel,
            Elements::parse(&[70, 1, 0]).unwrap()
        )
        .is_err()
    );
    assert!(
        Observation::new(
            *observation(9, -60).record(),
            channel,
            Elements::parse(&[127, 0, 127, 0]).unwrap()
        )
        .is_err()
    );
}
