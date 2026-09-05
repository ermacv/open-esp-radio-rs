pub(crate) fn internet_checksum(parts: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    let mut high = None;
    for part in parts {
        for byte in part.iter().copied() {
            if let Some(first) = high.take() {
                sum += u32::from(u16::from_be_bytes([first, byte]));
            } else {
                high = Some(byte);
            }
        }
    }
    if let Some(byte) = high {
        sum += u32::from(u16::from_be_bytes([byte, 0]));
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub(crate) fn udp_ipv4_checksum(
    source: [u8; 4],
    destination: [u8; 4],
    udp_header: &[u8],
    payload: &[u8],
) -> u16 {
    let udp_length = u16::try_from(udp_header.len() + payload.len())
        .expect("research UDP frame length is validated before checksum");
    let pseudo = [
        source[0],
        source[1],
        source[2],
        source[3],
        destination[0],
        destination[1],
        destination[2],
        destination[3],
        0,
        17,
        (udp_length >> 8) as u8,
        udp_length as u8,
    ];
    let checksum = internet_checksum(&[&pseudo, udp_header, payload]);
    if checksum == 0 { 0xffff } else { checksum }
}

#[cfg(test)]
mod tests;
