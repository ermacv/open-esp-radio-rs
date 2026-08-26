//! Concrete ESP32-S31 owner used by the shared station-attempt transaction.
//!
//! This target binding composes the already-qualified channel, join, peer and
//! WPA2 ports. Board code supplies coherent resource groups and a value-only
//! observer type; it does not sequence protocol or hardware phases.

use core::{future::Future, marker::PhantomData};

use open_esp_radio_esp32s31_hal::{RadioRuntimeOwner, phy_i2c::PhyI2cMasterControl};
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};
use open_esp_radio_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio_esp32s31_wifi_mac::{
    crypto::CcmpKeyHardware,
    he::He20PeerHardware,
    init::{StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate_control::BeamformingReportHardware,
    rx::RxDma,
    tx::TxHardware,
};
use open_esp_radio_esp32s31_wifi_sta::{
    attempt::{
        Esp32s31StaAttemptConnected, Esp32s31StaAttemptPort, Esp32s31StaAttemptReport,
        Esp32s31StaAttemptSecurity, Esp32s31StaAttemptSecurityExecution,
        Esp32s31StaAttemptStateError, Esp32s31StaAttemptStation, Esp32s31StaAttemptStepError,
        Esp32s31StaConnectedEntryFailure, Esp32s31StaInstalledSecurity,
    },
    channel::Esp32s31ScanPhy,
    join::{Esp32s31StaJoinObserver, Esp32s31StaJoinPortError, Esp32s31StaJoinTransmit},
    peer::{
        Esp32s31ConnectedStaPeer, Esp32s31PreparedStaPeer, Esp32s31ProgrammedStaPeer,
        Esp32s31StaPeerPort, Esp32s31StaPeerPortError, Esp32s31StaPeerRadio,
        Esp32s31StaPeerStation, Esp32s31StaPeerTransmit,
    },
    wpa2::{
        Esp32s31InstalledWpa2Keys, Esp32s31Wpa2HandshakePort, Esp32s31Wpa2HandshakePortError,
        Esp32s31Wpa2HandshakeRadio, Esp32s31Wpa2HandshakeStorage, Esp32s31Wpa2KeyPort,
        Esp32s31Wpa2KeyPortError, Esp32s31Wpa2KeyRadio, Esp32s31Wpa2KeySession,
        Esp32s31Wpa2Station, Esp32s31Wpa2Transmit,
    },
};
use open_esp_radio_ieee80211::security::WifiSecurityMode;
use open_esp_radio_ieee80211::station::{
    AssociationResponse, StaSecurityError, select_sta_association, select_wpa2_psk_rsn,
};
use open_esp_radio_wifi_sta::{
    join::{StaJoinError, StaJoinRunner},
    station::StaFailureDisposition,
};
use open_esp_radio_wpa2::{
    aes::{SoftwareAesKeyUnwrapError, Wpa2SoftwareAes},
    runner::{
        Wpa2Established, Wpa2HandshakeConfig, Wpa2HandshakeError, Wpa2HandshakeRunner,
        Wpa2KeyInstallError, Wpa2KeyInstallRunner, Wpa2PendingKeyInstall,
    },
};

use crate::{
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::{
        Esp32s31RxFrontier, Esp32s31RxFrontierDelay, Esp32s31RxFrontierError,
    },
    roles::station::join_port::{
        Esp32s31StaJoinPort, Esp32s31StaJoinRadio, Esp32s31StaJoinRx, Esp32s31StaJoinStation,
        Esp32s31StaJoinStorage,
    },
    roles::station::join_time::EmbassyStaJoinTimer,
    roles::station::wpa2_port::Esp32s31Wpa2Rx,
    roles::station::wpa2_time::EmbassyWpa2HandshakeTimer,
};

mod channel;
mod owner;
mod port;
mod resources;
mod service;

pub use channel::Esp32s31StaAttemptChannel;
pub use owner::{Esp32s31StaAttemptTargetError, Esp32s31StaAttemptTargetOwner};
pub use port::Esp32s31StaAttemptTargetPort;
pub use resources::{Esp32s31StaAttemptRadio, Esp32s31StaAttemptStorage};
