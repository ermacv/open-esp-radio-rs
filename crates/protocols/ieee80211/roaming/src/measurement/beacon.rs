//! Beacon channel selection over caller-supplied regulatory/backend facts.
use super::MeasurementError;
use crate::Body;
use core::mem::size_of;
use oer_ieee80211_mac::roaming::{
    BeaconMeasurementReport, BeaconMeasurementRequest, ELEMENT_HEADER_LEN, ELEMENT_LENGTH_OFFSET,
    Elements, MAX_ELEMENT_BODY_LEN, MeasurementReportElement, WireError, beacon_measurement_mode,
    beacon_report_subelement_id, beacon_reporting_detail, beacon_requested_channel, element_id,
    measurement_subelement_id,
};
// Timestamp, beacon interval, capability information.
const BEACON_FIXED_BODY_LEN: usize = size_of::<u64>() + 2 * size_of::<u16>();
// Measurement Pilot omits the Timestamp field and has its own fixed fields.
const PILOT_FIXED_BODY_LEN: usize = 8;
const MAX_REPORTED_FRAME_SUBELEMENT_LEN: usize = MAX_ELEMENT_BODY_LEN
    - MeasurementReportElement::FIXED_BODY_LEN
    - BeaconMeasurementReport::FIXED_BODY_LEN;
const MAX_REPORTED_FRAME_BODY_LEN: usize = MAX_REPORTED_FRAME_SUBELEMENT_LEN - ELEMENT_HEADER_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportedBeaconFrame<'a> {
    BeaconOrProbe {
        fixed: &'a [u8; BEACON_FIXED_BODY_LEN],
        elements: Elements<'a>,
    },
    MeasurementPilot {
        fixed: &'a [u8; PILOT_FIXED_BODY_LEN],
        elements: Elements<'a>,
    },
}
pub struct ReportedBeaconBody {
    body: Option<Body<MAX_REPORTED_FRAME_SUBELEMENT_LEN>>,
    pub omitted_elements: usize,
}
impl ReportedBeaconBody {
    pub fn subelement(&self) -> Option<&[u8]> {
        self.body.as_ref().map(Body::bytes)
    }
}

/// Apply Reporting Detail and the Request element to a complete observed frame.
/// As in the base Beacon Report procedure, the 255-octet Measurement Report
/// limit ends the body at a complete IE. `omitted_elements` exposes that mandated
/// wire limit; smaller local capacities never silently truncate a report.
/// `other_subelements` reserves space for all caller-supplied extensions.
pub fn reported_beacon_body(
    request: BeaconMeasurementRequest<'_>,
    frame: ReportedBeaconFrame<'_>,
    other_subelements: Elements<'_>,
) -> Result<ReportedBeaconBody, MeasurementError> {
    if other_subelements
        .unique(beacon_report_subelement_id::REPORTED_FRAME_BODY)?
        .is_some()
    {
        return Err(
            WireError::DuplicateElement(beacon_report_subelement_id::REPORTED_FRAME_BODY).into(),
        );
    }
    let detail = request
        .channel
        .subelements
        .unique(measurement_subelement_id::REPORTING_DETAIL)?;
    if detail.is_some_and(|body| body.len() != size_of::<u8>()) {
        return Err(MeasurementError::UnsupportedExtension(
            measurement_subelement_id::REPORTING_DETAIL,
        ));
    }
    let detail = detail.map_or(beacon_reporting_detail::ALL_ELEMENTS, |body| body[0]);
    if !beacon_reporting_detail::supported(detail) {
        return Err(MeasurementError::UnsupportedDetail(detail));
    }
    if detail == beacon_reporting_detail::NONE {
        return Ok(ReportedBeaconBody {
            body: None,
            omitted_elements: 0,
        });
    }
    let requested = request
        .channel
        .subelements
        .unique(measurement_subelement_id::REQUESTED_ELEMENTS)?;
    let (fixed, elements): (&[u8], Elements<'_>) = match frame {
        ReportedBeaconFrame::BeaconOrProbe { fixed, elements } => (fixed, elements),
        ReportedBeaconFrame::MeasurementPilot { fixed, elements } => (fixed, elements),
    };
    let capacity = MAX_REPORTED_FRAME_BODY_LEN
        .checked_sub(other_subelements.as_bytes().len())
        .ok_or(MeasurementError::ReportBodyFull)?;
    if capacity < fixed.len() {
        return Err(MeasurementError::ReportBodyFull);
    }
    let mut body = Body::<MAX_REPORTED_FRAME_SUBELEMENT_LEN>::empty();
    body.bytes[..ELEMENT_HEADER_LEN]
        .copy_from_slice(&[beacon_report_subelement_id::REPORTED_FRAME_BODY, 0]);
    body.bytes[ELEMENT_HEADER_LEN..ELEMENT_HEADER_LEN + fixed.len()].copy_from_slice(fixed);
    body.len = ELEMENT_HEADER_LEN + fixed.len();
    let mut ended = false;
    let mut omitted = 0;
    for element in elements.iter().filter(|element| {
        detail == beacon_reporting_detail::ALL_ELEMENTS
            || requested.is_some_and(|ids| ids.contains(&element.id))
    }) {
        let len = ELEMENT_HEADER_LEN + element.body.len();
        if ended || body.len - ELEMENT_HEADER_LEN + len > capacity {
            ended = true;
            omitted += 1;
            continue;
        }
        body.bytes[body.len..body.len + ELEMENT_HEADER_LEN]
            .copy_from_slice(&[element.id, element.body.len() as u8]);
        body.bytes[body.len + ELEMENT_HEADER_LEN..body.len + len].copy_from_slice(element.body);
        body.len += len;
    }
    body.bytes[ELEMENT_LENGTH_OFFSET] = (body.len - ELEMENT_HEADER_LEN) as u8;
    Ok(ReportedBeaconBody {
        body: Some(body),
        omitted_elements: omitted,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasurementChannel {
    pub operating_class: u8,
    pub channel: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermittedMeasurementChannel {
    pub channel: MeasurementChannel,
    pub passive: bool,
    pub active: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BeaconChannels<const CHANNELS: usize> {
    /// Channel/duration fields are ignored; read the existing beacon table.
    Table,
    Measure {
        channels: [Option<MeasurementChannel>; CHANNELS],
    },
}

/// `permitted` is the complete currently supported regulatory channel set;
/// `serving_ap_reports` comes from the latest serving-AP beacon. No operating
/// class/channel mapping or active-scan permission is guessed by this core.
pub fn beacon_channels<const CHANNELS: usize>(
    request: BeaconMeasurementRequest<'_>,
    permitted: &[PermittedMeasurementChannel],
    serving_ap_reports: Elements<'_>,
) -> Result<BeaconChannels<CHANNELS>, MeasurementError> {
    for (index, channel) in permitted.iter().enumerate() {
        if matches!(
            channel.channel.channel,
            beacon_requested_channel::ALL_IN_OPERATING_CLASS
                | beacon_requested_channel::AP_CHANNEL_REPORT
        ) || permitted[..index]
            .iter()
            .any(|other| other.channel == channel.channel)
        {
            return Err(MeasurementError::ConflictingChannels);
        }
    }
    if request.measurement_mode == beacon_measurement_mode::TABLE {
        return Ok(BeaconChannels::Table);
    }
    if !beacon_measurement_mode::supported(request.measurement_mode) {
        return Err(MeasurementError::UnsupportedBeaconMode(
            request.measurement_mode,
        ));
    }
    let mut channels = [None; CHANNELS];
    let mut count = 0;
    let mut push = |channel: MeasurementChannel| -> Result<(), MeasurementError> {
        if channels[..count].contains(&Some(channel)) {
            return Ok(());
        }
        let Some(allowed) = permitted.iter().find(|allowed| allowed.channel == channel) else {
            return Ok(());
        };
        if if request.measurement_mode == beacon_measurement_mode::PASSIVE {
            !allowed.passive
        } else {
            !allowed.active
        } {
            return Ok(());
        }
        if count == CHANNELS {
            return Err(MeasurementError::ChannelStorageFull);
        }
        channels[count] = Some(channel);
        count += 1;
        Ok(())
    };
    match request.channel.channel {
        beacon_requested_channel::ALL_IN_OPERATING_CLASS => {
            for allowed in permitted.iter().filter(|allowed| {
                allowed.channel.operating_class == request.channel.operating_class
            }) {
                push(allowed.channel)?;
            }
        }
        beacon_requested_channel::AP_CHANNEL_REPORT => {}
        channel => push(MeasurementChannel {
            operating_class: request.channel.operating_class,
            channel,
        })?,
    }
    let explicit = request
        .channel
        .subelements
        .iter()
        .any(|element| element.id == element_id::AP_CHANNEL_REPORT);
    let reports =
        if explicit || request.channel.channel != beacon_requested_channel::AP_CHANNEL_REPORT {
            request.channel.subelements
        } else {
            serving_ap_reports
        };
    let mut found = false;
    for element in reports
        .iter()
        .filter(|element| element.id == element_id::AP_CHANNEL_REPORT)
    {
        let (&operating_class, list) =
            element
                .body
                .split_first()
                .ok_or(WireError::InvalidElementLength(
                    element_id::AP_CHANNEL_REPORT,
                ))?;
        found = true;
        for &channel in list {
            if matches!(
                channel,
                beacon_requested_channel::ALL_IN_OPERATING_CLASS
                    | beacon_requested_channel::AP_CHANNEL_REPORT
            ) {
                return Err(MeasurementError::ConflictingChannels);
            }
            push(MeasurementChannel {
                operating_class,
                channel,
            })?;
        }
    }
    if request.channel.channel == beacon_requested_channel::AP_CHANNEL_REPORT && !found {
        return Err(MeasurementError::MissingChannelReports);
    }
    if count == 0 {
        return Err(MeasurementError::NoPermittedChannels);
    }
    Ok(BeaconChannels::Measure { channels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_mac::roaming::ChannelMeasurementRequest;
    fn request(channel: u8) -> BeaconMeasurementRequest<'static> {
        BeaconMeasurementRequest {
            channel: ChannelMeasurementRequest {
                operating_class: 81,
                channel,
                randomization_tu: 0,
                duration_tu: 10,
                subelements: Elements::EMPTY,
            },
            measurement_mode: 0,
            bssid: [255; 6],
        }
    }
    fn permitted() -> [PermittedMeasurementChannel; 3] {
        [1, 6, 11].map(|channel| PermittedMeasurementChannel {
            channel: MeasurementChannel {
                operating_class: 81,
                channel,
            },
            passive: true,
            active: channel != 11,
        })
    }
    #[test]
    fn wildcards_use_complete_regulatory_inputs_and_latest_reports() {
        let BeaconChannels::Measure { channels } =
            beacon_channels::<3>(request(0), &permitted(), Elements::EMPTY).unwrap()
        else {
            panic!()
        };
        assert_eq!(channels.iter().flatten().count(), 3);
        assert_eq!(
            beacon_channels::<2>(request(0), &permitted(), Elements::EMPTY),
            Err(MeasurementError::ChannelStorageFull)
        );
        assert_eq!(
            beacon_channels::<3>(request(255), &permitted(), Elements::EMPTY),
            Err(MeasurementError::MissingChannelReports)
        );
        let reports = Elements::parse(&[51, 4, 81, 6, 11, 6]).unwrap();
        let mut active = request(255);
        active.measurement_mode = 1;
        let BeaconChannels::Measure { channels } =
            beacon_channels::<3>(active, &permitted(), reports).unwrap()
        else {
            panic!()
        };
        assert_eq!(channels, [Some(permitted()[1].channel), None, None]);
    }
    #[test]
    fn explicit_reports_append_channels_and_table_never_admits_scan() {
        let mut exact = request(1);
        exact.channel.subelements = Elements::parse(&[51, 3, 81, 6, 11]).unwrap();
        let BeaconChannels::Measure { channels } =
            beacon_channels::<3>(exact, &permitted(), Elements::EMPTY).unwrap()
        else {
            panic!()
        };
        assert_eq!(channels.iter().flatten().count(), 3);
        exact.measurement_mode = 2;
        exact.channel.channel = 255;
        assert_eq!(
            beacon_channels::<0>(exact, &[], Elements::EMPTY).unwrap(),
            BeaconChannels::Table
        );
    }

    #[test]
    fn report_detail_filters_in_original_order_and_wire_limit_ends_at_complete_ie() {
        let fixed = [0; 12];
        let elements = Elements::parse(&[0, 1, 7, 221, 2, 8, 9, 0, 1, 6]).unwrap();
        let frame = ReportedBeaconFrame::BeaconOrProbe {
            fixed: &fixed,
            elements,
        };
        let mut req = request(6);
        req.channel.subelements = Elements::parse(&[2, 1, 1, 10, 1, 0]).unwrap();
        let body = reported_beacon_body(req, frame, Elements::EMPTY).unwrap();
        assert_eq!(&body.subelement().unwrap()[14..], [0, 1, 7, 0, 1, 6]);
        assert_eq!(body.omitted_elements, 0);
        req.channel.subelements = Elements::parse(&[2, 1, 0]).unwrap();
        assert!(
            reported_beacon_body(req, frame, Elements::EMPTY)
                .unwrap()
                .subelement()
                .is_none()
        );
        let mut elements = std::vec![221, 200];
        elements.extend_from_slice(&[1; 200]);
        elements.extend_from_slice(&[0, 20]);
        elements.extend_from_slice(&[2; 20]);
        elements.extend_from_slice(&[1, 1, 3]);
        req.channel.subelements = Elements::EMPTY;
        let frame = ReportedBeaconFrame::BeaconOrProbe {
            fixed: &fixed,
            elements: Elements::parse(&elements).unwrap(),
        };
        let body = reported_beacon_body(req, frame, Elements::EMPTY).unwrap();
        assert_eq!(body.subelement().unwrap().len(), 216);
        assert_eq!(body.omitted_elements, 2);
        assert_eq!(
            Elements::parse(&body.subelement().unwrap()[14..])
                .unwrap()
                .iter()
                .count(),
            1
        );
    }
}
