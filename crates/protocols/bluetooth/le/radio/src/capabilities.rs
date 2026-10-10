//! What a backend serves, stated before any request.

use crate::{DataPdu, RadioRequest, RadioTiming, ScanFilterPolicy, ScanType, TestPhy};

/// One LE PHY.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LePhy {
    /// LE 1M.
    Le1M,
    /// LE 2M.
    Le2M,
    /// LE Coded, with either coding.
    LeCoded,
}

impl LePhy {
    const fn bit(self) -> u8 {
        match self {
            Self::Le1M => 1,
            Self::Le2M => 1 << 1,
            Self::LeCoded => 1 << 2,
        }
    }
}

impl TestPhy {
    /// The PHY the test runs on, without its coding.
    pub const fn phy(self) -> LePhy {
        match self {
            Self::Le1M => LePhy::Le1M,
            Self::Le2M => LePhy::Le2M,
            Self::LeCodedS8 | Self::LeCodedS2 => LePhy::LeCoded,
        }
    }
}

/// A set of LE PHYs.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct LePhys(u8);

impl LePhys {
    /// No PHY.
    pub const NONE: Self = Self(0);
    /// LE 1M alone.
    pub const LE_1M: Self = Self::NONE.with(LePhy::Le1M);
    /// LE 1M, LE 2M and LE Coded.
    pub const ALL: Self = Self::LE_1M.with(LePhy::Le2M).with(LePhy::LeCoded);

    /// This set and `phy`.
    #[must_use]
    pub const fn with(self, phy: LePhy) -> Self {
        Self(self.0 | phy.bit())
    }

    /// Whether the set holds `phy`.
    pub const fn contains(self, phy: LePhy) -> bool {
        self.0 & phy.bit() != 0
    }
}

/// Who runs the Link Layer acknowledgement and flow control of a connection
/// (Core Specification Vol 6, Part B, 4.5.9).
///
/// The owner keeps the transmit sequence number (SN) and the next expected
/// sequence number (NESN), retransmits an unacknowledged PDU, suppresses
/// duplicates, answers with empty PDUs when nothing is queued and sets the
/// More Data (MD) bit.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkAcknowledgement {
    /// The radio hardware. The backend receives only the LLID and payload of
    /// each queued PDU, reports [`crate::RadioOutcome::TransmitAcknowledged`]
    /// when the peer acknowledged it and reports only new, non-empty
    /// receptions.
    Hardware,
    /// Backend software in its time-critical context, for example with
    /// `oer-bluetooth-ll`'s `connection::acknowledgement` state machine. The
    /// port contract is the same as for [`Self::Hardware`].
    Software,
}

/// How a backend carries a connection's data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LeConnectionCapabilities {
    /// Longest data PDU payload the backend transmits and receives, in
    /// octets, including an encrypted PDU's MIC. The protocol maximum is 255:
    /// 251 octets of data and the 4-octet MIC.
    pub max_data_payload: u8,
    /// Who acknowledges data PDUs.
    pub link_acknowledgement: LinkAcknowledgement,
}

/// What a backend serves.
///
/// A backend refuses every request these capabilities exclude with
/// [`crate::RequestError::Unsupported`] ([`Self::supports`] decides); a
/// request they include may still be refused for its resources or timing.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LeRadioCapabilities {
    /// Legacy advertising sets.
    pub legacy_advertising: bool,
    /// Passive scanners.
    pub passive_scanning: bool,
    /// Active scanners, which send `SCAN_REQ`.
    pub active_scanning: bool,
    /// A filter accept list and scanners that filter against it.
    pub filter_accept_list: bool,
    /// Peripheral connections, and how they carry data.
    pub peripheral_connection: Option<LeConnectionCapabilities>,
    /// Direct Test Mode.
    pub direct_test_mode: bool,
    /// PHYs of advertising sets, scanners and connections.
    pub phys: LePhys,
    /// PHYs of Direct Test Mode events.
    pub test_phys: LePhys,
    /// How the backend keeps reservations apart and admits events, which
    /// the planner uses.
    pub timing: RadioTiming,
}

impl LeRadioCapabilities {
    /// Nothing is served.
    pub const NONE: Self = Self {
        legacy_advertising: false,
        passive_scanning: false,
        active_scanning: false,
        filter_accept_list: false,
        peripheral_connection: None,
        direct_test_mode: false,
        phys: LePhys::NONE,
        test_phys: LePhys::NONE,
        timing: RadioTiming::MINIMAL,
    };

    /// Whether these capabilities include `request`.
    pub fn supports(&self, request: &RadioRequest<'_>) -> bool {
        let scanning = self.passive_scanning || self.active_scanning;
        let connection = self.peripheral_connection.is_some();
        match request {
            RadioRequest::ConfigureAdvertising(configuration) => {
                self.legacy_advertising && self.phys.contains(configuration.phy)
            }
            RadioRequest::Advertise(_) | RadioRequest::RemoveAdvertising(_) => {
                self.legacy_advertising
            }
            RadioRequest::ConfigureScanner(configuration) => {
                let scan_type = match configuration.scan_type {
                    ScanType::Passive => self.passive_scanning,
                    ScanType::Active => self.active_scanning,
                };
                let filter = match configuration.filter_policy {
                    ScanFilterPolicy::AcceptAll => true,
                    ScanFilterPolicy::AcceptListOnly => self.filter_accept_list,
                };
                scan_type && filter && self.phys.contains(configuration.phy)
            }
            RadioRequest::Scan(_) | RadioRequest::RemoveScanner(_) => scanning,
            RadioRequest::OpenConnection(configuration) => {
                connection && self.phys.contains(configuration.phy)
            }
            RadioRequest::ConnectionEvent(_) | RadioRequest::CloseConnection(_) => connection,
            RadioRequest::Transmit { pdu, .. } => self.carries(*pdu),
            RadioRequest::TestTransmit(test) => {
                self.direct_test_mode && self.test_phys.contains(test.phy.phy())
            }
            RadioRequest::TestReceive(test) => {
                self.direct_test_mode && self.test_phys.contains(test.phy.phy())
            }
            RadioRequest::EndTest => self.direct_test_mode,
            RadioRequest::FilterAcceptList(_) => self.filter_accept_list,
        }
    }

    fn carries(&self, pdu: DataPdu<'_>) -> bool {
        self.peripheral_connection.is_some_and(|connection| {
            pdu.payload().len() <= usize::from(connection.max_data_payload)
        })
    }
}

#[cfg(test)]
mod tests;
