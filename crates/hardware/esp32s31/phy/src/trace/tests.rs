use super::*;

use oer_phy_trace::{Client, Refusal};

use crate::state::client::PhyModemClient;

#[test]
fn every_client_keeps_its_identity() {
    assert_eq!(client(PhyModemClient::Wifi), Client::Wifi);
    assert_eq!(client(PhyModemClient::Bluetooth), Client::Bluetooth);
    assert_eq!(client(PhyModemClient::Ieee802154), Client::Ieee802154);
}

#[test]
fn refusals_name_the_client_they_concern() {
    assert_eq!(
        refusal(ConcurrentPhyError::ClientAbsent(PhyModemClient::Ieee802154)),
        Refusal::ClientAbsent(Client::Ieee802154)
    );
    assert_eq!(
        refusal(ConcurrentPhyError::MissingQuiescence(
            PhyModemClient::Bluetooth
        )),
        Refusal::MissingQuiescence(Client::Bluetooth)
    );
    assert_eq!(refusal(ConcurrentPhyError::RfClosed), Refusal::RfClosed);
    assert_eq!(
        refusal(ConcurrentPhyError::WindowClosed),
        Refusal::WindowClosed
    );
}

#[test]
fn calibration_paths_keep_the_cache_outcome() {
    assert_eq!(
        calibration_path(PhyCalibrationPath::FullAfterRejectedCache),
        CalibrationPath::FullAfterRejectedCache
    );
    assert_eq!(
        calibration_path(PhyCalibrationPath::PartialFromCache),
        CalibrationPath::PartialFromCache
    );
}
