//! Station authentication, disconnect and action frame codecs.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenAuthenticationRequest {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
}

impl OpenAuthenticationRequest {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        let required = MANAGEMENT_HEADER_LEN + 6;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            OPEN_AUTHENTICATION_FRAME_CONTROL,
            self.bssid,
            self.source,
            self.bssid,
            self.sequence_number,
        );
        frame[24..26].copy_from_slice(&OPEN_SYSTEM_ALGORITHM.to_le_bytes());
        frame[26..28].copy_from_slice(&OPEN_SYSTEM_REQUEST_SEQUENCE.to_le_bytes());
        frame[28..30].copy_from_slice(&0_u16.to_le_bytes());
        Ok(required)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenAuthenticationResponse {
    pub status_code: u16,
}

/// Management transition by which the selected AP ends the STA relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaDisconnectKind {
    Disassociation,
    Deauthentication,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDisconnect {
    pub kind: StaDisconnectKind,
    pub reason_code: u16,
}

/// One unprotected STA-originated Action management frame.
///
/// BlockAck negotiation uses this common 24-byte management header followed
/// by the nine-byte action body owned by the MAC BlockAck state machine.
///
/// `SOURCE[PROMOTED_RX_AMPDU]`: reviewed promoted ADDBA response builder,
/// where the same header was constructed around
/// `rx_ampdu::write_successful_addba_response`; the frame-control subtype is
/// the IEEE 802.11 Action management subtype also parsed by
/// `libnet80211.a`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaActionFrame<'a> {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: u16,
    pub body: &'a [u8],
}

impl StaActionFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid, self.sequence_number)?;
        let required = MANAGEMENT_HEADER_LEN.checked_add(self.body.len()).ok_or(
            StationFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }

        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            ACTION_FRAME_CONTROL,
            self.bssid,
            self.source,
            self.bssid,
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..].copy_from_slice(self.body);
        Ok(required)
    }
}

/// Parse a response addressed to this station and BSSID.
///
/// `None` means that the frame is valid input but belongs to another
/// management exchange. This lets an RX loop ignore beacons and other peers
/// without treating them as protocol errors.
pub fn parse_open_authentication_response(
    frame: &[u8],
    local: [u8; 6],
    bssid: [u8; 6],
) -> Option<OpenAuthenticationResponse> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 6
        || read_u16(frame, 0)? & 0x00fc != OPEN_AUTHENTICATION_FRAME_CONTROL
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
        || read_u16(frame, 24)? != OPEN_SYSTEM_ALGORITHM
        || read_u16(frame, 26)? != OPEN_SYSTEM_RESPONSE_SEQUENCE
    {
        return None;
    }
    Some(OpenAuthenticationResponse {
        status_code: read_u16(frame, 28)?,
    })
}

/// Parse a Disassociation or Deauthentication sent by the selected AP.
///
/// An authentication wait must treat this as a protocol transition rather
/// than an absent response. In particular, an AP may reject the first Open
/// Authentication after a rapid station reset by first clearing its previous
/// relationship with a Deauthentication frame.
///
/// SOURCE: complete
/// `libnet80211.a[ieee80211_sta.o]::sta_recv_mgmt`: branches `.L723`
/// (Disassociation) and `.L729` (Deauthentication) read the reason code at
/// management-body offset zero (`frame + 24`) and immediately call
/// `ieee80211_sta_new_state(g_ic, 0, (reason << 8) | subtype)`.
pub fn parse_sta_disconnect(frame: &[u8], local: [u8; 6], bssid: [u8; 6]) -> Option<StaDisconnect> {
    if frame.len() < MANAGEMENT_HEADER_LEN + 2
        || frame[4..10] != local
        || frame[10..16] != bssid
        || frame[16..22] != bssid
    {
        return None;
    }

    let kind = match read_u16(frame, 0)? & 0x00fc {
        DISASSOCIATION_FRAME_CONTROL => StaDisconnectKind::Disassociation,
        DEAUTHENTICATION_FRAME_CONTROL => StaDisconnectKind::Deauthentication,
        _ => return None,
    };
    Some(StaDisconnect {
        kind,
        reason_code: read_u16(frame, MANAGEMENT_HEADER_LEN)?,
    })
}
