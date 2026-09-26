//! Typed 802.11 frames decoded from a monitor capture.
//!
//! Every air observer decodes its capture here, with one fixed tshark field
//! set, and analyzes the resulting frames in memory. Absent fields decode to
//! `None`; a present field that does not parse, a malformed record or an
//! invalid timestamp fails the whole capture instead of being skipped.

use std::{fmt, path::Path, process::Command, str::FromStr};

use oer_process::CommandExt as _;

use crate::Result;

/// One 48-bit IEEE MAC address.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    pub const BROADCAST: Self = Self([0xff; 6]);
}

impl FromStr for MacAddress {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let mut address = [0_u8; 6];
        let mut parts = value.split(':');
        for byte in &mut address {
            *byte = parts
                .next()
                .filter(|part| part.len() == 2)
                .and_then(|part| u8::from_str_radix(part, 16).ok())
                .ok_or_else(|| format!("invalid MAC address `{value}`"))?;
        }
        if parts.next().is_some() {
            return Err(format!("invalid MAC address `{value}`"));
        }
        Ok(Self(address))
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, f] = self.0;
        write!(formatter, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
    }
}

/// Frame control type and subtype, encoded as `type << 4 | subtype`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameKind(pub u16);

impl FrameKind {
    pub const PROBE_REQUEST: Self = Self(0x04);
    pub const PROBE_RESPONSE: Self = Self(0x05);
    pub const BEACON: Self = Self(0x08);
    pub const BLOCK_ACK: Self = Self(0x19);
    pub const RTS: Self = Self(0x1b);
    pub const CTS: Self = Self(0x1c);
    pub const ACK: Self = Self(0x1d);

    pub const fn is_data(self) -> bool {
        self.0 >> 4 == 2
    }
}

/// The PHY that carried the frame, as classified by the capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirPhy {
    /// Clause 15 DSSS (1 and 2 Mb/s).
    Dsss,
    /// Clause 16 HR/DSSS (5.5 and 11 Mb/s).
    HrDsss,
    /// Clause 17/18 OFDM, including ERP-OFDM.
    Ofdm,
    Ht,
    Vht,
    He,
    Other(u8),
}

impl AirPhy {
    fn from_capture(value: u8) -> Self {
        match value {
            3 => Self::Dsss,
            4 => Self::HrDsss,
            5 | 6 => Self::Ofdm,
            7 => Self::Ht,
            8 => Self::Vht,
            11 => Self::He,
            other => Self::Other(other),
        }
    }

    /// DSSS and HR/DSSS rates; ERP stations must decode these.
    pub const fn is_dsss(self) -> bool {
        matches!(self, Self::Dsss | Self::HrDsss)
    }
}

/// Compressed BlockAck starting sequence and 64-bit bitmap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockAckBitmap {
    pub start_sequence: u16,
    pub bitmap: [u8; 8],
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AirFrame {
    pub time_micros: u64,
    pub kind: FrameKind,
    pub transmitter: Option<MacAddress>,
    pub receiver: Option<MacAddress>,
    pub destination: Option<MacAddress>,
    /// The MAC header Duration/ID field for a non-PS-Poll frame.
    pub duration_micros: Option<u16>,
    pub sequence: Option<u16>,
    pub fragment: Option<u8>,
    pub tid: Option<u8>,
    pub retry: Option<bool>,
    /// Transmitter-reported retries from a TX-monitor radiotap header.
    pub transmit_retries: Option<u32>,
    pub phy: Option<AirPhy>,
    pub rate_kbps: Option<u32>,
    pub block_ack: Option<BlockAckBitmap>,
    /// MAC time (TSFT) of the first bit of the MPDU, when the capture has it.
    pub mac_time_micros: Option<u64>,
    /// A DSSS/HR PPDU used the short PLCP preamble.
    pub short_preamble: Option<bool>,
    /// ERP Information element payload of a beacon or probe response.
    pub erp_information: Option<u8>,
    /// HT Protection field of an HT Operation element.
    pub ht_protection: Option<u8>,
    /// Frame body bytes, decoded only when requested.
    pub payload: Option<Vec<u8>>,
}

/// Whether to decode frame bodies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Payload {
    Omit,
    Include,
}

const FIELDS: [&str; 20] = [
    "frame.time_epoch",
    "wlan.fc.type_subtype",
    "wlan.ta",
    "wlan.ra",
    "wlan.da",
    "wlan.duration",
    "wlan.seq",
    "wlan.frag",
    "wlan.qos.tid",
    "wlan.fc.retry",
    "radiotap.data_retries",
    "wlan_radio.phy",
    "wlan_radio.data_rate",
    "wlan.fixed.ssc.sequence",
    "wlan.ba.bm",
    "radiotap.mactime",
    "radiotap.flags.preamble",
    "wlan.erp_info",
    "wlan.ht.info.ht_protection",
    "data.data",
];

/// Decode every frame of `path` that matches the display `filter`.
pub fn decode(path: &Path, filter: &str, payload: Payload) -> Result<Vec<AirFrame>> {
    let fields = match payload {
        Payload::Include => &FIELDS[..],
        Payload::Omit => &FIELDS[..FIELDS.len() - 1],
    };
    let mut command = Command::new("tshark");
    command.arg("-r").arg(path).args([
        "-Y",
        filter,
        "-T",
        "fields",
        "-E",
        "separator=\t",
        "-E",
        "occurrence=f",
    ]);
    for field in fields {
        command.args(["-e", field]);
    }
    let output = command.supervised_output()?;
    if !output.status.success() {
        return Err(crate::fixture::Error::new(format!(
            "cannot decode air capture {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    parse(&String::from_utf8(output.stdout)?, payload)
}

/// Parse tshark field records in [`FIELDS`] order.
pub fn parse(text: &str, payload: Payload) -> Result<Vec<AirFrame>> {
    let columns = match payload {
        Payload::Include => FIELDS.len(),
        Payload::Omit => FIELDS.len() - 1,
    };
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != columns {
                return Err(
                    format!("air capture record has {} fields: {line}", fields.len()).into(),
                );
            }
            frame(&fields).map_err(|error| format!("{error}: {line}").into())
        })
        .collect()
}

fn frame(fields: &[&str]) -> Result<AirFrame> {
    let start_sequence = optional::<u16>(fields[13])?;
    let bitmap = present(fields[14]).map(decode_bitmap).transpose()?;
    Ok(AirFrame {
        time_micros: epoch_micros(fields[0]).ok_or("invalid air capture timestamp")?,
        kind: FrameKind(
            present(fields[1])
                .map(parse_integer)
                .transpose()?
                .ok_or("air capture record has no frame type")?,
        ),
        transmitter: address(fields[2])?,
        receiver: address(fields[3])?,
        destination: address(fields[4])?,
        duration_micros: optional(fields[5])?,
        sequence: optional(fields[6])?,
        fragment: optional(fields[7])?,
        tid: optional(fields[8])?,
        retry: present(fields[9]).map(flag).transpose()?,
        transmit_retries: optional(fields[10])?,
        phy: present(fields[11])
            .map(parse_integer)
            .transpose()?
            .map(AirPhy::from_capture),
        rate_kbps: present(fields[12]).map(rate_kbps).transpose()?,
        block_ack: start_sequence
            .zip(bitmap)
            .map(|(start_sequence, bitmap)| BlockAckBitmap {
                start_sequence,
                bitmap,
            }),
        mac_time_micros: optional(fields[15])?,
        short_preamble: present(fields[16]).map(flag).transpose()?,
        erp_information: optional(fields[17])?,
        ht_protection: optional(fields[18])?,
        payload: fields
            .get(19)
            .and_then(|value| present(value))
            .map(decode_hex)
            .transpose()?,
    })
}

fn present(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn optional<T: TryFrom<u64>>(value: &str) -> Result<Option<T>> {
    present(value)
        .map(|value| {
            T::try_from(parse_integer::<u64>(value)?)
                .map_err(|_| format!("air capture field `{value}` is out of range").into())
        })
        .transpose()
}

fn parse_integer<T: TryFrom<u64>>(value: &str) -> Result<T> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse(),
    }
    .map_err(|_| format!("invalid air capture integer `{value}`"))?;
    T::try_from(parsed).map_err(|_| format!("air capture integer `{value}` is out of range").into())
}

fn address(value: &str) -> Result<Option<MacAddress>> {
    Ok(present(value).map(str::parse).transpose()?)
}

fn flag(value: &str) -> Result<bool> {
    match value {
        "1" | "True" | "true" => Ok(true),
        "0" | "False" | "false" => Ok(false),
        other => Err(format!("invalid air capture flag `{other}`").into()),
    }
}

/// Decimal Mb/s, as the capture reports it, in kb/s.
fn rate_kbps(value: &str) -> Result<u32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let mut kbps = whole
        .parse::<u32>()
        .ok()
        .and_then(|whole| whole.checked_mul(1_000));
    let mut scale = 100;
    for digit in fraction.bytes() {
        if !digit.is_ascii_digit() {
            kbps = None;
            break;
        }
        kbps = kbps.and_then(|kbps| kbps.checked_add(u32::from(digit - b'0') * scale));
        scale /= 10;
    }
    kbps.ok_or_else(|| format!("invalid air capture rate `{value}`").into())
}

fn decode_bitmap(value: &str) -> Result<[u8; 8]> {
    let bytes = decode_hex(value)?;
    bytes
        .try_into()
        .map_err(|_| format!("BlockAck bitmap `{value}` is not compressed").into())
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(format!("odd-length hexadecimal field `{value}`").into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| format!("invalid hexadecimal field `{value}`").into())
        })
        .collect()
}

/// Seconds with up to microsecond precision; additional digits truncate.
pub fn epoch_micros(value: &str) -> Option<u64> {
    let value = value.trim();
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    let seconds = seconds.parse::<u64>().ok()?;
    let mut micros = 0_u64;
    let mut digits = 0_u8;
    for byte in fraction.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        if digits < 6 {
            micros = micros * 10 + u64::from(byte - b'0');
            digits += 1;
        }
    }
    while digits < 6 {
        micros *= 10;
        digits += 1;
    }
    seconds.checked_mul(1_000_000)?.checked_add(micros)
}

#[cfg(test)]
pub(crate) mod tests;
