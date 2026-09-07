//! Station power-management signalling on the IEEE 802.11 wire.
//!
//! This module constructs the Null Data frame by which an associated station
//! tells its access point that its Power Management mode changed and the
//! legacy PS-Poll control frame used to retrieve one buffered unicast MPDU.
//! It owns no timer, retry, sleep, MMIO or executor state. In particular,
//! merely encoding either frame never grants permission to stop the radio:
//! the runtime must retain the corresponding acknowledged exchange.

use crate::station::{StationFrameError, validate_peer};

/// Length of a legacy Null Data MPDU, excluding the hardware-owned FCS.
pub const STA_NULL_DATA_FRAME_LEN: usize = 24;
/// Length of a legacy PS-Poll control MPDU, excluding the hardware-owned FCS.
pub const STA_PS_POLL_FRAME_LEN: usize = 16;
/// Largest Association ID assigned to an infrastructure BSS station.
pub const STA_MAX_ASSOCIATION_ID: u16 = 2_007;

const NULL_DATA_TO_DS_FRAME_CONTROL: u16 = 0x0148;
const PS_POLL_FRAME_CONTROL: u16 = 0x00a4;
const PS_POLL_ASSOCIATION_ID_PREFIX: u16 = 0xc000;
const POWER_MANAGEMENT_BIT: u16 = 0x1000;

/// Nonzero association identifier carried by a legacy PS-Poll frame.
///
/// The Duration/ID field reserves fourteen bits for the encoded value, but an
/// infrastructure BSS may assign only IDs 1 through 2007. Keeping the larger
/// reserved range out of this type prevents an invalid Association Response
/// from becoming a valid-looking PS-Poll transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationId(u16);

impl StaAssociationId {
    pub const fn new(value: u16) -> Option<Self> {
        if value != 0 && value <= STA_MAX_ASSOCIATION_ID {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    const fn duration_id(self) -> u16 {
        PS_POLL_ASSOCIATION_ID_PREFIX | self.0
    }
}

/// Power-management state advertised by a station to its access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPowerManagement {
    /// The station remains continuously available to receive frames.
    Active,
    /// The access point must buffer traffic while the station sleeps.
    PowerSave,
}

/// Complete inputs for one station-originated legacy Null Data frame.
///
/// The address geometry is fixed by the To-DS data-frame form: Address 1 and
/// Address 3 are the BSSID, while Address 2 is the station transmitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaNullDataFrame {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
    pub power_management: StaPowerManagement,
}

impl StaNullDataFrame {
    /// Encode the MPDU without an FCS.
    ///
    /// This is the same standard frame shape exposed by Linux mac80211's
    /// `ieee80211_nullfunc_get`: Data/NullFunc + ToDS, BSSID/STA/BSSID address
    /// geometry, with the caller selecting the Power Management bit.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        if output.len() < STA_NULL_DATA_FRAME_LEN {
            return Err(StationFrameError::OutputTooSmall {
                required: STA_NULL_DATA_FRAME_LEN,
            });
        }

        let frame = &mut output[..STA_NULL_DATA_FRAME_LEN];
        frame.fill(0);
        let frame_control = NULL_DATA_TO_DS_FRAME_CONTROL
            | if matches!(self.power_management, StaPowerManagement::PowerSave) {
                POWER_MANAGEMENT_BIT
            } else {
                0
            };
        frame[0..2].copy_from_slice(&frame_control.to_le_bytes());
        frame[4..10].copy_from_slice(&self.bssid);
        frame[10..16].copy_from_slice(&self.station_address);
        frame[16..22].copy_from_slice(&self.bssid);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        Ok(STA_NULL_DATA_FRAME_LEN)
    }
}

/// Complete inputs for one station-originated legacy PS-Poll frame.
///
/// IEEE control-frame geometry places the BSSID in Address 1 and the station
/// transmitter address in Address 2. PS-Poll has no sequence-control field;
/// its Duration/ID field carries the associated station's validated AID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPsPollFrame {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub association_id: StaAssociationId,
}

impl StaPsPollFrame {
    /// Encode the complete control MPDU without an FCS.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, 0)?;
        if output.len() < STA_PS_POLL_FRAME_LEN {
            return Err(StationFrameError::OutputTooSmall {
                required: STA_PS_POLL_FRAME_LEN,
            });
        }

        let frame = &mut output[..STA_PS_POLL_FRAME_LEN];
        frame.fill(0);
        frame[0..2].copy_from_slice(&PS_POLL_FRAME_CONTROL.to_le_bytes());
        frame[2..4].copy_from_slice(&self.association_id.duration_id().to_le_bytes());
        frame[4..10].copy_from_slice(&self.bssid);
        frame[10..16].copy_from_slice(&self.station_address);
        Ok(STA_PS_POLL_FRAME_LEN)
    }
}

#[cfg(test)]
mod tests;
