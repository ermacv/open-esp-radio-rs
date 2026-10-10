use super::{LeConnectionCapabilities, LePhy, LePhys, LeRadioCapabilities, LinkAcknowledgement};
use crate::{
    AdvertisingConfiguration, AdvertisingPdu, AdvertisingReception, AdvertisingSetId, DataPdu,
    DataPduKind, EventId, LeInstant, LeWindow, RadioDuration, RadioRequest, ScanFilterPolicy,
    ScanType, ScannerConfiguration, ScannerId, TestChannel, TestPhy, TestReceive, TxPower,
};

const ADVERTISING_PDU: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];

fn le_1m_peripheral() -> LeRadioCapabilities {
    LeRadioCapabilities {
        legacy_advertising: true,
        passive_scanning: true,
        active_scanning: false,
        filter_accept_list: false,
        peripheral_connection: Some(LeConnectionCapabilities {
            max_data_payload: 27,
            link_acknowledgement: LinkAcknowledgement::Hardware,
        }),
        direct_test_mode: true,
        phys: LePhys::LE_1M,
        test_phys: LePhys::LE_1M.with(LePhy::Le2M),
        timing: crate::RadioTiming::MINIMAL,
    }
}

fn advertising(phy: LePhy) -> RadioRequest<'static> {
    RadioRequest::ConfigureAdvertising(AdvertisingConfiguration {
        set: AdvertisingSetId::new(0),
        pdu: AdvertisingPdu::new(&ADVERTISING_PDU).unwrap(),
        reception: AdvertisingReception::None,
        tx_power: TxPower::from_dbm(0),
        phy,
    })
}

fn scanner(scan_type: ScanType, filter_policy: ScanFilterPolicy) -> RadioRequest<'static> {
    RadioRequest::ConfigureScanner(ScannerConfiguration {
        scanner: ScannerId::new(0),
        scan_type,
        filter_policy,
        tx_power: TxPower::from_dbm(0),
        phy: LePhy::Le1M,
    })
}

fn test_receive(phy: TestPhy) -> RadioRequest<'static> {
    RadioRequest::TestReceive(TestReceive {
        id: EventId::new(1),
        channel: TestChannel::new(0).unwrap(),
        phy,
        window: LeWindow::new(
            LeInstant::from_micros(1_000),
            RadioDuration::from_micros(100),
        )
        .unwrap(),
        recurring: false,
        tx_power: TxPower::from_dbm(0),
    })
}

#[test]
fn phy_sets_hold_what_they_were_built_with() {
    assert!(LePhys::LE_1M.contains(LePhy::Le1M));
    assert!(!LePhys::LE_1M.contains(LePhy::Le2M));
    assert!(!LePhys::NONE.contains(LePhy::Le1M));
    for phy in [LePhy::Le1M, LePhy::Le2M, LePhy::LeCoded] {
        assert!(LePhys::ALL.contains(phy));
    }
    assert_eq!(TestPhy::LeCodedS2.phy(), LePhy::LeCoded);
    assert_eq!(TestPhy::LeCodedS8.phy(), LePhy::LeCoded);
}

#[test]
fn role_configuration_needs_its_role_and_phy() {
    let capabilities = le_1m_peripheral();
    assert!(capabilities.supports(&advertising(LePhy::Le1M)));
    assert!(!capabilities.supports(&advertising(LePhy::Le2M)));
    assert!(capabilities.supports(&scanner(ScanType::Passive, ScanFilterPolicy::AcceptAll)));
    assert!(!capabilities.supports(&scanner(ScanType::Active, ScanFilterPolicy::AcceptAll)));
    assert!(!capabilities.supports(&scanner(
        ScanType::Passive,
        ScanFilterPolicy::AcceptListOnly
    )));
}

#[test]
fn direct_test_mode_checks_the_test_phy() {
    let capabilities = le_1m_peripheral();
    assert!(capabilities.supports(&test_receive(TestPhy::Le2M)));
    assert!(!capabilities.supports(&test_receive(TestPhy::LeCodedS8)));
    assert!(!LeRadioCapabilities::NONE.supports(&test_receive(TestPhy::Le1M)));
    assert!(!LeRadioCapabilities::NONE.supports(&RadioRequest::EndTest));
}

#[test]
fn transmissions_are_bounded_by_the_connection_payload() {
    let capabilities = le_1m_peripheral();
    let transmit = |payload| RadioRequest::Transmit {
        connection: crate::ConnectionId::new(0),
        pdu: DataPdu::new(DataPduKind::Start, payload).unwrap(),
    };
    assert!(capabilities.supports(&transmit(&[0; 27])));
    assert!(!capabilities.supports(&transmit(&[0; 28])));
    assert!(!LeRadioCapabilities::NONE.supports(&transmit(&[])));
}
