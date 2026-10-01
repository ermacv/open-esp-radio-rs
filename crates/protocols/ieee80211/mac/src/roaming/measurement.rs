//! RRM bodies and measurement elements. TSF stays in the requesting BSS epoch;
//! all durations/randomization windows here are TU, not beacon intervals.

use super::codec::{BodyReader, BodyWriter};
use super::{
    ACTION_HEADER_LEN, DIALOG_TOKEN_OFFSET, ELEMENT_HEADER_LEN, Elements, MAC_ADDRESS_LEN,
    MAX_ELEMENT_BODY_LEN, MacAddress, OCTET_VALUE_COUNT, RADIO_MEASUREMENT_CATEGORY, RrmAction,
    WireError, element_id, header, output, token,
};
use crate::management::MAX_SSID_LEN;
use core::mem::size_of;
const CHANNEL_REQUEST_FIXED_LEN: usize = 2 * size_of::<u8>() + 2 * size_of::<u16>();
const BEACON_REQUEST_FIXED_LEN: usize =
    CHANNEL_REQUEST_FIXED_LEN + size_of::<u8>() + MAC_ADDRESS_LEN;
const MEASUREMENT_FIXED_LEN: usize = 3 * size_of::<u8>();
const REPORT_HEADER_LEN: usize = 2 * size_of::<u8>() + size_of::<u64>() + size_of::<u16>();
const CHANNEL_LOAD_FIXED_LEN: usize = REPORT_HEADER_LEN + size_of::<u8>();
pub const NOISE_HISTOGRAM_BIN_COUNT: usize = 11;
const NOISE_HISTOGRAM_FIXED_LEN: usize =
    REPORT_HEADER_LEN + 2 * size_of::<u8>() + NOISE_HISTOGRAM_BIN_COUNT;
const BEACON_REPORT_FIXED_LEN: usize =
    REPORT_HEADER_LEN + 4 * size_of::<u8>() + MAC_ADDRESS_LEN + size_of::<u32>();
const RADIO_REQUEST_FIXED_LEN: usize = ACTION_HEADER_LEN + size_of::<u16>();
const LINK_REQUEST_FIXED_LEN: usize = ACTION_HEADER_LEN + 2 * size_of::<i8>();
const TPC_REPORT_BODY_LEN: usize = size_of::<i8>() + size_of::<u8>();
const LINK_REPORT_FIXED_LEN: usize =
    ACTION_HEADER_LEN + ELEMENT_HEADER_LEN + TPC_REPORT_BODY_LEN + 4 * size_of::<u8>();
pub mod measurement_subelement_id {
    pub const REPORTING_INFORMATION: u8 = 1;
    pub const REPORTING_DETAIL: u8 = 2;
    pub const REQUESTED_ELEMENTS: u8 = 10;
    pub const LAST_BEACON_REPORT_INDICATION: u8 = 164;
}
pub mod beacon_measurement_mode {
    pub const PASSIVE: u8 = 0;
    pub const ACTIVE: u8 = 1;
    pub const TABLE: u8 = 2;
    pub const fn supported(value: u8) -> bool {
        matches!(value, PASSIVE | ACTIVE | TABLE)
    }
}

pub const INDEFINITE_MEASUREMENT_REPETITIONS: u16 = u16::MAX;

pub const MEASUREMENT_REQUEST_ELEMENT_ID: u8 = element_id::MEASUREMENT_REQUEST;
pub const MEASUREMENT_REPORT_ELEMENT_ID: u8 = element_id::MEASUREMENT_REPORT;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementType(pub u8);
impl MeasurementType {
    pub const BASIC: Self = Self(0);
    pub const CCA: Self = Self(1);
    pub const RPI_HISTOGRAM: Self = Self(2);
    pub const fn radio_management(self) -> bool {
        !matches!(self, Self::BASIC | Self::CCA | Self::RPI_HISTOGRAM)
    }
    pub const CHANNEL_LOAD: Self = Self(3);
    pub const NOISE_HISTOGRAM: Self = Self(4);
    pub const BEACON: Self = Self(5);
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementRequestMode(pub u8);
impl MeasurementRequestMode {
    pub const PARALLEL: Self = Self(1 << 0);
    pub const CONTROL: Self = Self(1 << 1);
    pub const REQUESTS_ALLOWED: Self = Self(1 << 2);
    pub const REPORTS_ALLOWED: Self = Self(1 << 3);
    pub const DURATION_MANDATORY: Self = Self(1 << 4);
    pub const KNOWN_BITS: u8 = Self::PARALLEL.0
        | Self::CONTROL.0
        | Self::REQUESTS_ALLOWED.0
        | Self::REPORTS_ALLOWED.0
        | Self::DURATION_MANDATORY.0;
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    pub const fn parallel(self) -> bool {
        self.contains(Self::PARALLEL)
    }
    pub const fn control(self) -> bool {
        self.contains(Self::CONTROL)
    }
    pub const fn requests_allowed(self) -> bool {
        self.contains(Self::REQUESTS_ALLOWED)
    }
    pub const fn reports_allowed(self) -> bool {
        self.contains(Self::REPORTS_ALLOWED)
    }
    pub const fn duration_mandatory(self) -> bool {
        self.contains(Self::DURATION_MANDATORY)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementReportMode(pub u8);
impl MeasurementReportMode {
    pub const ACCEPT: Self = Self(0);
    pub const LATE: Self = Self(1);
    pub const INCAPABLE: Self = Self(2);
    pub const REFUSED: Self = Self(4);
    pub const fn rejected(self) -> bool {
        self.0 & (Self::LATE.0 | Self::INCAPABLE.0 | Self::REFUSED.0) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelMeasurementRequest<'a> {
    pub operating_class: u8,
    pub channel: u8,
    pub randomization_tu: u16,
    pub duration_tu: u16,
    pub subelements: Elements<'a>,
}
impl<'a> ChannelMeasurementRequest<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        let operating_class = fields.u8()?;
        let channel = fields.u8()?;
        let randomization_tu = u16::from_le_bytes(fields.array()?);
        let duration_tu = u16::from_le_bytes(fields.array()?);
        let value = Self {
            operating_class,
            channel,
            randomization_tu,
            duration_tu,
            subelements: Elements::parse(fields.remaining())?,
        };
        value.reporting_condition()?;
        Ok(value)
    }
    /// Reporting condition and reference value, if present. Assigned/unknown
    /// condition values remain visible for capability admission by the owner.
    pub fn reporting_condition(self) -> Result<Option<(u8, u8)>, WireError> {
        match self
            .subelements
            .unique(measurement_subelement_id::REPORTING_INFORMATION)?
        {
            Some(body) if body.len() >= 2 * size_of::<u8>() => Ok(Some((body[0], body[1]))),
            Some(_) => Err(WireError::InvalidElementLength(
                measurement_subelement_id::REPORTING_INFORMATION,
            )),
            None => Ok(None),
        }
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.reporting_condition()?;
        let len = CHANNEL_REQUEST_FIXED_LEN + self.subelements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[self.operating_class, self.channel]);
        fields.put(&self.randomization_tu.to_le_bytes());
        fields.put(&self.duration_tu.to_le_bytes());
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconMeasurementRequest<'a> {
    pub channel: ChannelMeasurementRequest<'a>,
    /// Passive=0, active=1, table=2; later/reserved octets are retained.
    pub measurement_mode: u8,
    pub bssid: MacAddress,
}
impl<'a> BeaconMeasurementRequest<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        let operating_class = fields.u8()?;
        let channel = fields.u8()?;
        let randomization_tu = u16::from_le_bytes(fields.array()?);
        let duration_tu = u16::from_le_bytes(fields.array()?);
        let measurement_mode = fields.u8()?;
        let bssid = fields.array()?;
        let subelements = Elements::parse(fields.remaining())?;
        validate_beacon_request(subelements)?;
        Ok(Self {
            channel: ChannelMeasurementRequest {
                operating_class,
                channel,
                randomization_tu,
                duration_tu,
                subelements,
            },
            measurement_mode,
            bssid,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        validate_beacon_request(self.channel.subelements)?;
        let len = BEACON_REQUEST_FIXED_LEN + self.channel.subelements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[self.channel.operating_class, self.channel.channel]);
        fields.put(&self.channel.randomization_tu.to_le_bytes());
        fields.put(&self.channel.duration_tu.to_le_bytes());
        fields.put(&[self.measurement_mode]);
        fields.put(&self.bssid);
        fields.put(self.channel.subelements.as_bytes());
        Ok(len)
    }
}
fn validate_beacon_request(elements: Elements<'_>) -> Result<(), WireError> {
    for (id, length) in [
        (
            measurement_subelement_id::REPORTING_INFORMATION,
            2 * size_of::<u8>(),
        ),
        (measurement_subelement_id::REPORTING_DETAIL, size_of::<u8>()),
    ] {
        if elements.unique(id)?.is_some_and(|body| body.len() < length) {
            return Err(WireError::InvalidElementLength(id));
        }
    }
    if elements
        .unique(measurement_subelement_id::LAST_BEACON_REPORT_INDICATION)?
        .is_some_and(|body| body.len() != 1)
    {
        return Err(WireError::InvalidElementLength(
            measurement_subelement_id::LAST_BEACON_REPORT_INDICATION,
        ));
    }
    if elements
        .unique(element_id::SSID)?
        .is_some_and(|body| body.len() > MAX_SSID_LEN)
    {
        return Err(WireError::SsidTooLong);
    }
    elements.unique(measurement_subelement_id::REQUESTED_ELEMENTS)?;
    for element in elements
        .iter()
        .filter(|element| element.id == element_id::AP_CHANNEL_REPORT)
    {
        if element.body.is_empty() {
            return Err(WireError::InvalidElementLength(
                element_id::AP_CHANNEL_REPORT,
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasurementRequestData<'a> {
    Control,
    ChannelLoad(ChannelMeasurementRequest<'a>),
    NoiseHistogram(ChannelMeasurementRequest<'a>),
    Beacon(BeaconMeasurementRequest<'a>),
    Other(&'a [u8]),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementRequestElement<'a> {
    pub measurement_token: u8,
    pub mode: MeasurementRequestMode,
    pub measurement_type: MeasurementType,
    pub body: &'a [u8],
}
impl<'a> MeasurementRequestElement<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        let value = Self {
            measurement_token: fields.u8()?,
            mode: MeasurementRequestMode(fields.u8()?),
            measurement_type: MeasurementType(fields.u8()?),
            body: fields.remaining(),
        };
        value.data()?;
        Ok(value)
    }
    pub fn data(self) -> Result<MeasurementRequestData<'a>, WireError> {
        if self.measurement_token == 0 {
            return Err(WireError::ZeroMeasurementToken);
        }
        if self.mode.control() && self.body.is_empty() {
            return Ok(MeasurementRequestData::Control);
        }
        Ok(match self.measurement_type {
            MeasurementType::CHANNEL_LOAD => {
                MeasurementRequestData::ChannelLoad(ChannelMeasurementRequest::parse(self.body)?)
            }
            MeasurementType::NOISE_HISTOGRAM => {
                MeasurementRequestData::NoiseHistogram(ChannelMeasurementRequest::parse(self.body)?)
            }
            MeasurementType::BEACON => {
                MeasurementRequestData::Beacon(BeaconMeasurementRequest::parse(self.body)?)
            }
            _ => MeasurementRequestData::Other(self.body),
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.data()?;
        encode_element(
            buffer,
            MEASUREMENT_REQUEST_ELEMENT_ID,
            self.measurement_token,
            self.mode.0,
            self.measurement_type,
            self.body,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementReportHeader {
    pub operating_class: u8,
    pub channel: u8,
    pub start_tsf: u64,
    pub duration_tu: u16,
}
impl MeasurementReportHeader {
    fn parse(body: &[u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        Ok(Self {
            operating_class: fields.u8()?,
            channel: fields.u8()?,
            start_tsf: u64::from_le_bytes(fields.array()?),
            duration_tu: u16::from_le_bytes(fields.array()?),
        })
    }
    fn write(self, out: &mut [u8]) {
        let mut fields = BodyWriter::new(out);
        fields.put(&[self.operating_class, self.channel]);
        fields.put(&self.start_tsf.to_le_bytes());
        fields.put(&self.duration_tu.to_le_bytes());
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelLoadReport<'a> {
    pub header: MeasurementReportHeader,
    pub load: u8,
    pub subelements: Elements<'a>,
}
impl<'a> ChannelLoadReport<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let header = MeasurementReportHeader::parse(body)?;
        let mut fields = BodyReader::new(body);
        fields.take(REPORT_HEADER_LEN)?;
        Ok(Self {
            header,
            load: fields.u8()?,
            subelements: Elements::parse(fields.remaining())?,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let len = CHANNEL_LOAD_FIXED_LEN + self.subelements.as_bytes().len();
        let out = output(buffer, len)?;
        self.header.write(out);
        let mut fields = BodyWriter::new(&mut out[REPORT_HEADER_LEN..]);
        fields.put(&[self.load]);
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoiseHistogramReport<'a> {
    pub header: MeasurementReportHeader,
    pub antenna_id: u8,
    pub anpi: u8,
    pub ipi_density: [u8; NOISE_HISTOGRAM_BIN_COUNT],
    pub subelements: Elements<'a>,
}
impl<'a> NoiseHistogramReport<'a> {
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let header = MeasurementReportHeader::parse(body)?;
        let mut fields = BodyReader::new(body);
        fields.take(REPORT_HEADER_LEN)?;
        Ok(Self {
            header,
            antenna_id: fields.u8()?,
            anpi: fields.u8()?,
            ipi_density: fields.array()?,
            subelements: Elements::parse(fields.remaining())?,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let len = NOISE_HISTOGRAM_FIXED_LEN + self.subelements.as_bytes().len();
        let out = output(buffer, len)?;
        self.header.write(out);
        let mut fields = BodyWriter::new(&mut out[REPORT_HEADER_LEN..]);
        fields.put(&[self.antenna_id, self.anpi]);
        fields.put(&self.ipi_density);
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconMeasurementReport<'a> {
    pub header: MeasurementReportHeader,
    pub frame_information: u8,
    pub rcpi: u8,
    pub rsni: u8,
    pub bssid: MacAddress,
    pub antenna_id: u8,
    pub parent_tsf: u32,
    pub subelements: Elements<'a>,
}
impl<'a> BeaconMeasurementReport<'a> {
    pub const FIXED_BODY_LEN: usize = BEACON_REPORT_FIXED_LEN;
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let header = MeasurementReportHeader::parse(body)?;
        let mut fields = BodyReader::new(body);
        fields.take(REPORT_HEADER_LEN)?;
        Ok(Self {
            header,
            frame_information: fields.u8()?,
            rcpi: fields.u8()?,
            rsni: fields.u8()?,
            bssid: fields.array()?,
            antenna_id: fields.u8()?,
            parent_tsf: u32::from_le_bytes(fields.array()?),
            subelements: Elements::parse(fields.remaining())?,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        let len = BEACON_REPORT_FIXED_LEN + self.subelements.as_bytes().len();
        let out = output(buffer, len)?;
        self.header.write(out);
        let mut fields = BodyWriter::new(&mut out[REPORT_HEADER_LEN..]);
        fields.put(&[self.frame_information, self.rcpi, self.rsni]);
        fields.put(&self.bssid);
        fields.put(&[self.antenna_id]);
        fields.put(&self.parent_tsf.to_le_bytes());
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasurementReportData<'a> {
    Rejected,
    EmptyBeacon,
    ChannelLoad(ChannelLoadReport<'a>),
    NoiseHistogram(NoiseHistogramReport<'a>),
    Beacon(BeaconMeasurementReport<'a>),
    Other(&'a [u8]),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementReportElement<'a> {
    pub measurement_token: u8,
    pub mode: MeasurementReportMode,
    pub measurement_type: MeasurementType,
    pub body: &'a [u8],
}
impl<'a> MeasurementReportElement<'a> {
    pub const FIXED_BODY_LEN: usize = MEASUREMENT_FIXED_LEN;
    pub fn parse(body: &'a [u8]) -> Result<Self, WireError> {
        let mut fields = BodyReader::new(body);
        let value = Self {
            measurement_token: fields.u8()?,
            mode: MeasurementReportMode(fields.u8()?),
            measurement_type: MeasurementType(fields.u8()?),
            body: fields.remaining(),
        };
        value.data()?;
        Ok(value)
    }
    pub fn data(self) -> Result<MeasurementReportData<'a>, WireError> {
        if self.mode.rejected() {
            return if self.body.is_empty() {
                Ok(MeasurementReportData::Rejected)
            } else {
                Err(WireError::InconsistentFields)
            };
        }
        Ok(match self.measurement_type {
            MeasurementType::CHANNEL_LOAD => {
                MeasurementReportData::ChannelLoad(ChannelLoadReport::parse(self.body)?)
            }
            MeasurementType::NOISE_HISTOGRAM => {
                MeasurementReportData::NoiseHistogram(NoiseHistogramReport::parse(self.body)?)
            }
            MeasurementType::BEACON if self.body.is_empty() => MeasurementReportData::EmptyBeacon,
            MeasurementType::BEACON => {
                MeasurementReportData::Beacon(BeaconMeasurementReport::parse(self.body)?)
            }
            _ => MeasurementReportData::Other(self.body),
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.data()?;
        encode_element(
            buffer,
            MEASUREMENT_REPORT_ELEMENT_ID,
            self.measurement_token,
            self.mode.0,
            self.measurement_type,
            self.body,
        )
    }
}
fn encode_element(
    buffer: &mut [u8],
    id: u8,
    token: u8,
    mode: u8,
    kind: MeasurementType,
    body: &[u8],
) -> Result<usize, WireError> {
    if body.len() > MAX_ELEMENT_BODY_LEN - MEASUREMENT_FIXED_LEN {
        return Err(WireError::ElementTooLong);
    }
    let len = ELEMENT_HEADER_LEN + MEASUREMENT_FIXED_LEN + body.len();
    let out = output(buffer, len)?;
    out[..ELEMENT_HEADER_LEN + MEASUREMENT_FIXED_LEN].copy_from_slice(&[
        id,
        (MEASUREMENT_FIXED_LEN + body.len()) as u8,
        token,
        mode,
        kind.0,
    ]);
    out[ELEMENT_HEADER_LEN + MEASUREMENT_FIXED_LEN..].copy_from_slice(body);
    Ok(len)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioMeasurementRequest<'a> {
    pub dialog_token: u8,
    /// 0 executes once; 65535 requests indefinite repetitions.
    pub repetitions: u16,
    pub elements: Elements<'a>,
}
impl<'a> RadioMeasurementRequest<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(
            bytes,
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::MeasurementRequest as u8,
            RADIO_REQUEST_FIXED_LEN,
        )?;
        let mut fields = BodyReader::new(bytes);
        fields.take(DIALOG_TOKEN_OFFSET)?;
        let value = Self {
            dialog_token: fields.u8()?,
            repetitions: u16::from_le_bytes(fields.array()?),
            elements: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn measurements(
        self,
    ) -> Result<impl Iterator<Item = MeasurementRequestElement<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|element| element.id == MEASUREMENT_REQUEST_ELEMENT_ID)
            .map(|element| {
                MeasurementRequestElement::parse(element.body).expect("validated request")
            }))
    }
    pub fn validate(self) -> Result<(), WireError> {
        token(self.dialog_token)?;
        let mut seen = [false; OCTET_VALUE_COUNT];
        for element in self
            .elements
            .iter()
            .filter(|element| element.id == MEASUREMENT_REQUEST_ELEMENT_ID)
        {
            let measurement = MeasurementRequestElement::parse(element.body)?;
            if !measurement.measurement_type.radio_management() {
                return Err(WireError::InconsistentFields);
            }
            if core::mem::replace(&mut seen[usize::from(measurement.measurement_token)], true) {
                return Err(WireError::DuplicateMeasurementToken(
                    measurement.measurement_token,
                ));
            }
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = RADIO_REQUEST_FIXED_LEN + self.elements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::MeasurementRequest as u8,
            self.dialog_token,
        ]);
        fields.put(&self.repetitions.to_le_bytes());
        fields.put(self.elements.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioMeasurementReport<'a> {
    pub dialog_token: u8,
    pub elements: Elements<'a>,
}
impl<'a> RadioMeasurementReport<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        if bytes.len() < ACTION_HEADER_LEN {
            return Err(WireError::Truncated);
        }
        let mut fields = BodyReader::new(bytes);
        if fields.u8()? != RADIO_MEASUREMENT_CATEGORY
            || fields.u8()? != RrmAction::MeasurementReport as u8
        {
            return Err(WireError::WrongAction);
        }
        let value = Self {
            dialog_token: fields.u8()?,
            elements: Elements::parse(fields.remaining())?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn reports(self) -> Result<impl Iterator<Item = MeasurementReportElement<'a>>, WireError> {
        self.validate()?;
        Ok(self
            .elements
            .iter()
            .filter(|element| element.id == MEASUREMENT_REPORT_ELEMENT_ID)
            .map(|element| {
                MeasurementReportElement::parse(element.body).expect("validated report")
            }))
    }
    pub fn validate(self) -> Result<(), WireError> {
        for element in self
            .elements
            .iter()
            .filter(|element| element.id == MEASUREMENT_REPORT_ELEMENT_ID)
        {
            if !MeasurementReportElement::parse(element.body)?
                .measurement_type
                .radio_management()
            {
                return Err(WireError::InconsistentFields);
            }
        }
        Ok(())
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        self.validate()?;
        let len = ACTION_HEADER_LEN + self.elements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::MeasurementReport as u8,
            self.dialog_token,
        ]);
        fields.put(self.elements.as_bytes());
        Ok(len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkMeasurementRequest<'a> {
    pub dialog_token: u8,
    pub transmit_power_dbm: i8,
    pub maximum_transmit_power_dbm: i8,
    pub subelements: Elements<'a>,
}
impl<'a> LinkMeasurementRequest<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(
            bytes,
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::LinkRequest as u8,
            LINK_REQUEST_FIXED_LEN,
        )?;
        let mut fields = BodyReader::new(bytes);
        fields.take(DIALOG_TOKEN_OFFSET)?;
        Ok(Self {
            dialog_token: fields.u8()?,
            transmit_power_dbm: fields.u8()? as i8,
            maximum_transmit_power_dbm: fields.u8()? as i8,
            subelements: Elements::parse(fields.remaining())?,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        token(self.dialog_token)?;
        let len = LINK_REQUEST_FIXED_LEN + self.subelements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::LinkRequest as u8,
            self.dialog_token,
            self.transmit_power_dbm as u8,
            self.maximum_transmit_power_dbm as u8,
        ]);
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkMeasurementReport<'a> {
    pub dialog_token: u8,
    pub transmit_power_dbm: i8,
    pub link_margin_db: u8,
    pub receive_antenna_id: u8,
    pub transmit_antenna_id: u8,
    pub rcpi: u8,
    pub rsni: u8,
    pub subelements: Elements<'a>,
}
impl<'a> LinkMeasurementReport<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        header(
            bytes,
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::LinkReport as u8,
            LINK_REPORT_FIXED_LEN,
        )?;
        let mut fields = BodyReader::new(bytes);
        fields.take(DIALOG_TOKEN_OFFSET)?;
        let dialog_token = fields.u8()?;
        if fields.u8()? != element_id::TPC_REPORT
            || usize::from(fields.u8()?) != TPC_REPORT_BODY_LEN
        {
            return Err(WireError::InvalidElementLength(element_id::TPC_REPORT));
        }
        Ok(Self {
            dialog_token,
            transmit_power_dbm: fields.u8()? as i8,
            link_margin_db: fields.u8()?,
            receive_antenna_id: fields.u8()?,
            transmit_antenna_id: fields.u8()?,
            rcpi: fields.u8()?,
            rsni: fields.u8()?,
            subelements: Elements::parse(fields.remaining())?,
        })
    }
    pub fn encode(self, buffer: &mut [u8]) -> Result<usize, WireError> {
        token(self.dialog_token)?;
        let len = LINK_REPORT_FIXED_LEN + self.subelements.as_bytes().len();
        let mut fields = BodyWriter::new(output(buffer, len)?);
        fields.put(&[
            RADIO_MEASUREMENT_CATEGORY,
            RrmAction::LinkReport as u8,
            self.dialog_token,
            element_id::TPC_REPORT,
            TPC_REPORT_BODY_LEN as u8,
            self.transmit_power_dbm as u8,
            self.link_margin_db,
            self.receive_antenna_id,
            self.transmit_antenna_id,
            self.rcpi,
            self.rsni,
        ]);
        fields.put(self.subelements.as_bytes());
        Ok(len)
    }
}
#[cfg(test)]
mod tests;

/// Beacon Request Channel Number special values (8.4.2.21.7).
pub mod beacon_requested_channel {
    pub const ALL_IN_OPERATING_CLASS: u8 = 0;
    pub const AP_CHANNEL_REPORT: u8 = u8::MAX;
}
pub mod beacon_reporting_detail {
    pub const NONE: u8 = 0;
    pub const REQUESTED_ELEMENTS: u8 = 1;
    pub const ALL_ELEMENTS: u8 = 2;
    pub const fn supported(value: u8) -> bool {
        matches!(value, NONE | REQUESTED_ELEMENTS | ALL_ELEMENTS)
    }
}
pub mod beacon_report_subelement_id {
    pub const REPORTED_FRAME_BODY: u8 = 1;
}
pub mod channel_reporting_condition {
    pub const ALWAYS: u8 = 0;
    pub const AT_LEAST: u8 = 1;
    pub const AT_MOST: u8 = 2;
}
/// IEEE 802.11-2012 Beacon Reporting Information (8.4.2.21.7).
pub mod beacon_reporting_condition {
    pub const ALWAYS: u8 = 0;
    pub const RCPI_ABOVE: u8 = 1;
    pub const RCPI_BELOW: u8 = 2;
    pub const RSNI_ABOVE: u8 = 3;
    pub const RSNI_BELOW: u8 = 4;
    pub const RCPI_ABOVE_SERVING_OFFSET: u8 = 5;
    pub const RCPI_BELOW_SERVING_OFFSET: u8 = 6;
    pub const RSNI_ABOVE_SERVING_OFFSET: u8 = 7;
    pub const RSNI_BELOW_SERVING_OFFSET: u8 = 8;
    pub const RCPI_BETWEEN_SERVING_AND_OFFSET: u8 = 9;
    pub const RSNI_BETWEEN_SERVING_AND_OFFSET: u8 = 10;
}
pub const SIGNAL_MEASUREMENT_UNAVAILABLE: u8 = u8::MAX;
