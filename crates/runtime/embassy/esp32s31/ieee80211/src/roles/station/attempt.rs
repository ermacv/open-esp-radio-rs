//! Concrete ESP32-S31 owner used by the shared station-attempt transaction.
//!
//! This target binding composes the already-qualified channel, join, peer and
//! WPA2 ports. Board code supplies coherent resource groups and a value-only
//! observer type; it does not sequence protocol or hardware phases.

use core::{future::Future, marker::PhantomData};

use crate::{
    datapath::rx::{
        dma::ReceiveDmaStorage,
        frontier::{ReceiveFrontier, RxFrontierDelay, RxFrontierError},
    },
    roles::station::{
        join_port::{StaJoinPort, StaJoinRadio, StaJoinRx, StaJoinStation, StaJoinStorage},
        join_time::EmbassyStaJoinTimer,
        wpa2_port::Wpa2Rx,
        wpa2_time::EmbassyWpa2HandshakeTimer,
    },
};

use oer_esp32s31_hal::owner::RadioRuntimeOwner;

use oer_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};

use oer_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;

use oer_esp32s31_wifi_mac::{
    crypto::CcmpKeyHardware,
    he::He20PeerHardware,
    init::{MacRuntimeStopHardware, StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate::control::BeamformingReportHardware,
    rx::RxDma,
    tx::TxHardware,
};

use oer_esp32s31_wifi_sta::{
    attempt::{
        StaAttemptConnected, StaAttemptPort, StaAttemptReport, StaAttemptSecurity,
        StaAttemptSecurityExecution, StaAttemptStateError, StaAttemptStation, StaAttemptStepError,
        StaConnectedEntryFailure, StaInstalledSecurity,
    },
    hardware::channel::ScanPhy,
    join::{StaJoinObserver, StaJoinPortError, StaJoinTransmit},
    peer::{
        ConnectedStaPeer, PreparedStaPeer, ProgrammedStaPeer, StaPeerPort, StaPeerPortError,
        StaPeerRadio, StaPeerStation, StaPeerTransmit,
    },
    profile::select_association,
    wpa2::{
        HandshakeTransmit, InstalledWpa2Keys, Wpa2HandshakePort, Wpa2HandshakePortError,
        Wpa2HandshakeRadio, Wpa2HandshakeStorage, Wpa2KeyPort, Wpa2KeyPortError, Wpa2KeyRadio,
        Wpa2KeySession, Wpa2Station,
    },
};

use oer_ieee80211::{
    security::WifiSecurityMode,
    station::{AssociationResponse, StaSecurityError, select_wpa2_psk_rsn},
};

use oer_wifi_sta::{
    join::{StaJoinError, StaJoinRunner},
    station::StaFailureDisposition,
};

use oer_wpa2::{
    aes::{SoftwareAesKeyUnwrapError, Wpa2SoftwareAes},
    runner::{
        Wpa2Established, Wpa2HandshakeConfig, Wpa2HandshakeError, Wpa2HandshakeRunner,
        Wpa2KeyInstallError, Wpa2KeyInstallRunner, Wpa2PendingKeyInstall,
    },
};

mod channel;
mod owner;
mod port;
mod resources;
mod service;

pub use channel::StaAttemptChannel;

pub use owner::{StaAttemptTargetError, StaAttemptTargetOwner};

pub use port::StaAttemptTargetPort;

pub use resources::{StaAttemptRadio, StaAttemptStorage};
