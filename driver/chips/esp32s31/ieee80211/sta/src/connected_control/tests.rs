use super::*;

fn core() -> Esp32s31ConnectedControlCore {
    Esp32s31ConnectedControlCore::new(
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        true,
        StaTxBlockAckSessions::new(32, 100_000, true).unwrap(),
    )
}

#[test]
fn readiness_combines_owned_state_with_external_event_state() {
    let mut core = core();
    assert!(!core.has_immediate_work(false));
    assert!(core.has_immediate_work(true));

    core.initial_tx_block_ack[1] = true;
    core.tx_block_ack_attempts_remaining[1] = 1;
    assert!(core.has_immediate_work(false));
    assert!(core.has_pending_traffic(DatapathControlContext::IDLE, false));
}

#[test]
fn deadline_is_computed_without_an_executor_timer() {
    let mut core = core();
    assert_eq!(core.next_alarm_deadline(), None);

    core.tx_block_ack.begin(7, 23, 50).unwrap();
    assert_eq!(core.next_alarm_deadline(), Some(100_050));
}

#[test]
fn connected_ftm_request_is_consumed_at_hardware_frontier() {
    use open_esp_radio_ieee80211::ftm::{
        FtmBurstDuration, FtmFormatAndBandwidth, FtmRequestParameters,
    };

    let parameters = FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        4,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap();
    let config = FtmRequesterConfig::new(parameters, 1_000, 100, 10_000, 1).unwrap();
    let frontier = core().evaluate_ftm_request_frontier(config, 50).unwrap();
    assert_eq!(frontier.peer, [0x20, 0x21, 0x22, 0x23, 0x24, 0x25]);
    assert_eq!(frontier.attempt, 1);
    assert_eq!(
        frontier.protocol_event,
        FtmRequesterEvent::Failed(
            open_esp_radio_wifi_sta::ftm::FtmSessionFailure::HardwareAdmissionRejected
        )
    );
    assert_eq!(
        frontier.hardware_error,
        StationFtmHardwareError::Unsupported {
            reached: crate::ftm::StationFtmHardwareStage::PortableInitialRequestValidated,
            missing: crate::ftm::StationFtmUnsupportedStage::RuntimePhyOwnerBinding,
        }
    );
}
