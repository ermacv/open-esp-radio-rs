use super::{
    BluetoothDtmChannel, BluetoothDtmChannelError, BluetoothDtmPhy, BluetoothDtmPhyError,
    BluetoothDtmPhyRoleError,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmSchedulerReceiverPhy, BluetoothDtmSchedulerTransmitterPhy,
};

#[test]
fn channel_domain_accepts_its_bounds_and_rejects_the_first_outside_image() {
    assert!(BluetoothDtmChannel::new(0).is_ok());
    assert!(BluetoothDtmChannel::new(39).is_ok());
    assert_eq!(
        BluetoothDtmChannel::new(40),
        Err(BluetoothDtmChannelError::OutsideTestChannelDomain)
    );
}

#[test]
fn phy_role_domain_rejects_only_the_transmitter_only_receiver_case() {
    assert_eq!(
        BluetoothDtmPhy::Le1M.scheduler_transmitter_phy(),
        BluetoothDtmSchedulerTransmitterPhy::Le1M
    );
    assert_eq!(
        BluetoothDtmPhy::Le2M.scheduler_transmitter_phy(),
        BluetoothDtmSchedulerTransmitterPhy::Le2M
    );
    assert_eq!(
        BluetoothDtmPhy::LeCoded.scheduler_transmitter_phy(),
        BluetoothDtmSchedulerTransmitterPhy::LeCodedS8
    );
    assert_eq!(
        BluetoothDtmPhy::LeCodedS2.scheduler_transmitter_phy(),
        BluetoothDtmSchedulerTransmitterPhy::LeCodedS2
    );
    assert_eq!(
        BluetoothDtmPhy::Le1M.scheduler_receiver_phy(),
        Ok(BluetoothDtmSchedulerReceiverPhy::Le1M)
    );
    assert_eq!(
        BluetoothDtmPhy::Le2M.scheduler_receiver_phy(),
        Ok(BluetoothDtmSchedulerReceiverPhy::Le2M)
    );
    assert_eq!(
        BluetoothDtmPhy::LeCoded.scheduler_receiver_phy(),
        Ok(BluetoothDtmSchedulerReceiverPhy::LeCoded)
    );
    assert_eq!(
        BluetoothDtmPhy::LeCodedS2.scheduler_receiver_phy(),
        Err(BluetoothDtmPhyRoleError::LeCodedS2RequiresTransmitter)
    );
}

#[test]
fn hci_phy_decoder_accepts_only_the_reviewed_selector_domain() {
    assert_eq!(
        BluetoothDtmPhy::from_hci_selector(1),
        Ok(BluetoothDtmPhy::Le1M)
    );
    assert_eq!(
        BluetoothDtmPhy::from_hci_selector(2),
        Ok(BluetoothDtmPhy::Le2M)
    );
    assert_eq!(
        BluetoothDtmPhy::from_hci_selector(3),
        Ok(BluetoothDtmPhy::LeCoded)
    );
    assert_eq!(
        BluetoothDtmPhy::from_hci_selector(4),
        Ok(BluetoothDtmPhy::LeCodedS2)
    );
    assert_eq!(
        BluetoothDtmPhy::from_hci_selector(0),
        Err(BluetoothDtmPhyError::UnsupportedHciSelector)
    );
    assert_eq!(
        BluetoothDtmPhy::from_hci_selector(5),
        Err(BluetoothDtmPhyError::UnsupportedHciSelector)
    );
    assert_eq!(BluetoothDtmPhy::Le1M.hci_selector(), 1);
    assert_eq!(BluetoothDtmPhy::Le2M.hci_selector(), 2);
    assert_eq!(BluetoothDtmPhy::LeCoded.hci_selector(), 3);
    assert_eq!(BluetoothDtmPhy::LeCodedS2.hci_selector(), 4);
}
