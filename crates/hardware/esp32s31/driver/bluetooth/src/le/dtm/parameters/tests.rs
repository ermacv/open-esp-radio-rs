use oer_esp32s31_bluetooth_memory::{DtmSchedulerReceiverPhy, DtmSchedulerTransmitterPhy};

use super::{DtmChannel, DtmChannelError, DtmPhy, DtmPhyError, DtmPhyRoleError};

#[test]
fn channel_domain_accepts_its_bounds_and_rejects_the_first_outside_image() {
    assert!(DtmChannel::new(0).is_ok());
    assert!(DtmChannel::new(39).is_ok());
    assert_eq!(
        DtmChannel::new(40),
        Err(DtmChannelError::OutsideTestChannelDomain)
    );
}

#[test]
fn phy_role_domain_rejects_only_the_transmitter_only_receiver_case() {
    assert_eq!(
        DtmPhy::Le1M.scheduler_transmitter_phy(),
        DtmSchedulerTransmitterPhy::Le1M
    );
    assert_eq!(
        DtmPhy::Le2M.scheduler_transmitter_phy(),
        DtmSchedulerTransmitterPhy::Le2M
    );
    assert_eq!(
        DtmPhy::LeCoded.scheduler_transmitter_phy(),
        DtmSchedulerTransmitterPhy::LeCodedS8
    );
    assert_eq!(
        DtmPhy::LeCodedS2.scheduler_transmitter_phy(),
        DtmSchedulerTransmitterPhy::LeCodedS2
    );
    assert_eq!(
        DtmPhy::Le1M.scheduler_receiver_phy(),
        Ok(DtmSchedulerReceiverPhy::Le1M)
    );
    assert_eq!(
        DtmPhy::Le2M.scheduler_receiver_phy(),
        Ok(DtmSchedulerReceiverPhy::Le2M)
    );
    assert_eq!(
        DtmPhy::LeCoded.scheduler_receiver_phy(),
        Ok(DtmSchedulerReceiverPhy::LeCoded)
    );
    assert_eq!(
        DtmPhy::LeCodedS2.scheduler_receiver_phy(),
        Err(DtmPhyRoleError::LeCodedS2RequiresTransmitter)
    );
}

#[test]
fn hci_phy_decoder_accepts_only_the_reviewed_selector_domain() {
    assert_eq!(DtmPhy::from_hci_selector(1), Ok(DtmPhy::Le1M));
    assert_eq!(DtmPhy::from_hci_selector(2), Ok(DtmPhy::Le2M));
    assert_eq!(DtmPhy::from_hci_selector(3), Ok(DtmPhy::LeCoded));
    assert_eq!(DtmPhy::from_hci_selector(4), Ok(DtmPhy::LeCodedS2));
    assert_eq!(
        DtmPhy::from_hci_selector(0),
        Err(DtmPhyError::UnsupportedHciSelector)
    );
    assert_eq!(
        DtmPhy::from_hci_selector(5),
        Err(DtmPhyError::UnsupportedHciSelector)
    );
    assert_eq!(DtmPhy::Le1M.hci_selector(), 1);
    assert_eq!(DtmPhy::Le2M.hci_selector(), 2);
    assert_eq!(DtmPhy::LeCoded.hci_selector(), 3);
    assert_eq!(DtmPhy::LeCodedS2.hci_selector(), 4);
}
