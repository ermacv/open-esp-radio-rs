//! Authenticate accepted, duplicate-filtered RX packets before LL/Host dispatch.
//!
//! Empty acknowledgements carry no MIC and never cross into Host ACL storage.
//! They do not advance the encryption handshake or either packet counter.

use oer_bluetooth_ll::security::{
    LePeripheralEncryptedReceive as Received, LePeripheralEncryptionProcedure as Procedure,
    LePeripheralEncryptionProcedureError as Error, LePeripheralEncryptionRandom,
    LePeripheralEncryptionReceiveMode as Mode,
};

pub(super) fn decode<'a>(
    encryption: &mut Procedure,
    pdu: &'a [u8],
    decoded: &'a mut [u8; u8::MAX as usize + 2],
    random: impl FnMut() -> LePeripheralEncryptionRandom,
) -> Option<&'a [u8]> {
    // Failure is terminal even when a later packet in the same batch is empty.
    if encryption.termination_reason().is_some() {
        return None;
    }
    match decode_inner(encryption, pdu, decoded, random) {
        Ok(length) => length.map(|length| &decoded[..length]),
        Err(_) => {
            decoded.fill(0);
            encryption.fail_unexpected_physical_channel_pdu();
            None
        }
    }
}

fn decode_inner(
    encryption: &mut Procedure,
    pdu: &[u8],
    decoded: &mut [u8; u8::MAX as usize + 2],
    mut random: impl FnMut() -> LePeripheralEncryptionRandom,
) -> Result<Option<usize>, Error> {
    if pdu.len() < 2 || usize::from(pdu[1]) + 2 != pdu.len() {
        return Err(Error::UnexpectedPhysicalChannelPdu);
    }
    let header = pdu[0];
    let payload = &pdu[2..];
    // Core Vol 6, Part B 2.4.1 and 5.1.3: Empty PDUs remain unencrypted,
    // including while waiting for the Host LTK. No data is released here.
    if header & 0x03 == 0x01 && payload.is_empty() {
        return Ok(None);
    }
    decoded[..pdu.len()].copy_from_slice(pdu);
    match encryption.receive_mode() {
        Mode::Plaintext => {
            if header & 0x03 == 0x03 && payload.first() == Some(&0x03) {
                if payload.len() != 23 {
                    return Err(Error::MalformedEncryptionRequest);
                }
                encryption.begin(payload, random())?;
            } else {
                return Ok(Some(pdu.len()));
            }
        }
        Mode::EncryptedStartResponse => {
            encryption.receive_encrypted_start_response(header, &mut decoded[2..pdu.len()])?;
        }
        Mode::Encrypted => {
            match encryption.receive_active_packet(header, &mut decoded[2..pdu.len()])? {
                Received::PauseRequest => {}
                Received::Plaintext { length } => {
                    decoded[1] = length as u8;
                    return Ok(Some(length + 2));
                }
            }
        }
        Mode::UnencryptedPauseResponse => {
            if header & 0x03 != 0x03 {
                return Err(Error::UnexpectedPhysicalChannelPdu);
            }
            encryption.receive_unencrypted_pause_response(payload)?;
        }
        Mode::RestartEncryptionRequest => {
            if header & 0x03 != 0x03 || payload.len() != 23 || payload[0] != 0x03 {
                return Err(Error::UnexpectedPhysicalChannelPdu);
            }
            encryption.begin(payload, random())?;
        }
        Mode::Blocked => return Err(Error::UnexpectedPhysicalChannelPdu),
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
