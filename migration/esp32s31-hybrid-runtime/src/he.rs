//! Allocation-free parsing for the bounded 2.4-GHz HE20 capability prefix.
//!
//! This module does not enable HE transmission. It owns only the stateless
//! representation needed to qualify an AP before any peer or descriptor state
//! is mutated.

pub const HE_CAPABILITIES_EXTENSION_ID: u8 = 35;
pub const HE_OPERATION_EXTENSION_ID: u8 = 36;
pub const HE_CAPABILITIES_IE_MIN_LEN: usize = 24;
pub const HE_OPERATION_IE_MIN_LEN: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMcsNssSupport {
    Mcs0To7,
    Mcs0To9,
    Mcs0To11,
    NotSupported,
}

impl HeMcsNssSupport {
    const fn from_map(map: u16, spatial_stream: u8) -> Self {
        let shift = spatial_stream.saturating_sub(1) as u32 * 2;
        match (map >> shift) & 0x03 {
            0 => Self::Mcs0To7,
            1 => Self::Mcs0To9,
            2 => Self::Mcs0To11,
            _ => Self::NotSupported,
        }
    }

    pub const fn supports_mcs9(self) -> bool {
        matches!(self, Self::Mcs0To9 | Self::Mcs0To11)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Capabilities {
    pub receive_mcs_map: u16,
    pub transmit_mcs_map: u16,
    pub receive_nss1: HeMcsNssSupport,
    pub transmit_nss1: HeMcsNssSupport,
}

impl He20Capabilities {
    pub const fn supports_bidirectional_mcs9(self) -> bool {
        self.receive_nss1.supports_mcs9() && self.transmit_nss1.supports_mcs9()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Operation {
    pub bss_color: u8,
    pub basic_mcs_nss_map: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct He20PeerState {
    pub capability_prefix: [u8; HE_CAPABILITIES_IE_MIN_LEN],
    pub max_rate_code: u8,
    pub packet_padding_eight_us: u8,
    pub operation_parameters: u32,
    pub bss_color_information: u8,
    pub basic_mcs_nss_map: u16,
    pub rts_threshold: Option<u16>,
    pub extended_range_single_user: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeElementError {
    WrongElement,
    LengthMismatch,
    TooShort,
    WrongExtension,
}

fn validate_extension<'a>(
    element: &'a [u8],
    extension: u8,
    minimum_len: usize,
) -> Result<&'a [u8], HeElementError> {
    if element.first().copied() != Some(255) {
        return Err(HeElementError::WrongElement);
    }
    let Some(declared_len) = element.get(1).copied() else {
        return Err(HeElementError::TooShort);
    };
    if usize::from(declared_len).checked_add(2) != Some(element.len()) {
        return Err(HeElementError::LengthMismatch);
    }
    if element.len() < minimum_len {
        return Err(HeElementError::TooShort);
    }
    if element.get(2).copied() != Some(extension) {
        return Err(HeElementError::WrongExtension);
    }
    Ok(element)
}

/// Parse the mandatory HE MAC/PHY and <=80-MHz MCS/NSS prefix.
///
/// The two maps are at fixed offsets before any optional wider-bandwidth or
/// PPE fields. Only NSS1 is interpreted because ESP32-S31 has one spatial
/// stream and this runtime is qualifying HE20.
pub fn parse_he20_capabilities(element: &[u8]) -> Result<He20Capabilities, HeElementError> {
    let element = validate_extension(
        element,
        HE_CAPABILITIES_EXTENSION_ID,
        HE_CAPABILITIES_IE_MIN_LEN,
    )?;
    let receive_mcs_map = u16::from_le_bytes([element[20], element[21]]);
    let transmit_mcs_map = u16::from_le_bytes([element[22], element[23]]);
    Ok(He20Capabilities {
        receive_mcs_map,
        transmit_mcs_map,
        receive_nss1: HeMcsNssSupport::from_map(receive_mcs_map, 1),
        transmit_nss1: HeMcsNssSupport::from_map(transmit_mcs_map, 1),
    })
}

/// Parse the mandatory HE Operation prefix.
///
/// Optional VHT/co-hosted/6-GHz tails are deliberately ignored. Their
/// presence is described by the operation parameters but none is needed for
/// the 2.4-GHz HE20 qualification boundary.
pub fn parse_he20_operation(element: &[u8]) -> Result<He20Operation, HeElementError> {
    let element = validate_extension(element, HE_OPERATION_EXTENSION_ID, HE_OPERATION_IE_MIN_LEN)?;
    Ok(He20Operation {
        bss_color: element[6] & 0x3f,
        basic_mcs_nss_map: u16::from_le_bytes([element[7], element[8]]),
    })
}

/// Recover the bounded peer state installed by the pinned HE capability and
/// operation parsers.
///
/// This is deliberately a pure transform. The caller separately owns the
/// node stores and the finite MMIO leaves, keeping validation outside the
/// small unsafe hardware boundary.
pub(crate) fn parse_he20_peer_state(
    capability: &[u8],
    operation: &[u8],
) -> Result<He20PeerState, HeElementError> {
    let capability = validate_extension(
        capability,
        HE_CAPABILITIES_EXTENSION_ID,
        HE_CAPABILITIES_IE_MIN_LEN,
    )?;
    let operation = validate_extension(
        operation,
        HE_OPERATION_EXTENSION_ID,
        HE_OPERATION_IE_MIN_LEN,
    )?;

    let mut capability_prefix = [0_u8; HE_CAPABILITIES_IE_MIN_LEN];
    capability_prefix.copy_from_slice(&capability[..HE_CAPABILITIES_IE_MIN_LEN]);

    // `ieee80211_parse_hecap` selects the MCS9 rate code only when the first
    // receive NSS map is above MCS0-7. Strict HE HIL advertises and accepts
    // precisely that one-stream contract.
    let max_rate_code = if capability[20] & 0x03 == 0 { 172 } else { 229 };

    // Without PPE thresholds the nominal packet-padding field is in the
    // mandatory PHY prefix. With PPE present, the pinned parser derives the
    // RU26/NSS1 value from the first two PPE bytes. The node stores units of
    // eight microseconds; the hardware leaf receives that value shifted by 3.
    let packet_padding_eight_us = if capability[15] & 0x80 == 0 {
        capability[18] >> 6
    } else {
        let ppe0 = *capability.get(24).ok_or(HeElementError::TooShort)?;
        let ppe1 = *capability.get(25).ok_or(HeElementError::TooShort)?;
        let ppet8 = ((ppe1 & 0x03) << 1) | (ppe0 >> 7);
        if ppe0 & 0x08 != 0 && ppe1 & 0x1c == 0x1c && ppet8 == 0 {
            2
        } else {
            0
        }
    };

    let operation_parameters =
        u32::from(operation[3]) | (u32::from(operation[4]) << 8) | (u32::from(operation[5]) << 16);
    let encoded_rts_threshold =
        (u16::from(operation[4] & 0x3f) << 4) | u16::from(operation[3] >> 4);
    let rts_threshold =
        (!matches!(encoded_rts_threshold, 0 | 0x03ff)).then_some(encoded_rts_threshold);

    Ok(He20PeerState {
        capability_prefix,
        max_rate_code,
        packet_padding_eight_us,
        operation_parameters,
        bss_color_information: operation[6],
        basic_mcs_nss_map: u16::from_le_bytes([operation[7], operation[8]]),
        rts_threshold,
        extended_range_single_user: operation[5] & 0x01 != 0,
    })
}

#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum He20HardwareError {
    InvalidAssociationId,
    UnsupportedRtsThreshold,
}

#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
unsafe extern "C" {
    static mut g_bss_color_collision_detection_enabled: u8;
}

#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
static DISABLED_BSS_COLOR_COLLISIONS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Number of stale hardware color-collision notifications consumed after the
/// interface-0 producer was disabled.
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
pub fn disabled_bss_color_collision_count() -> usize {
    DISABLED_BSS_COLOR_COLLISIONS.load(core::sync::atomic::Ordering::Acquire)
}

/// Consume event 30 only when HE collision reporting is already disabled.
///
/// The interrupt can publish one last event after the interface flag is
/// cleared. Calling the vendor consumer would allocate a robust-management
/// action frame. This bounded leaf instead acknowledges the hardware bitmap
/// and lets the Rust radio future continue. If reporting is enabled, it fails
/// closed so an unimplemented live collision-reporting path cannot be hidden.
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
#[link_section = ".rwtext.wifi_strict.he_peer"]
pub(crate) unsafe fn consume_disabled_bss_color_collision() -> bool {
    if core::ptr::addr_of!(g_bss_color_collision_detection_enabled).read_volatile() != 0 {
        return false;
    }

    const HE_BSS_COLOR_BITMAP_CONTROL: *mut u32 = 0x2010_4048 as *mut u32;
    HE_BSS_COLOR_BITMAP_CONTROL
        .write_volatile(HE_BSS_COLOR_BITMAP_CONTROL.read_volatile() | 0x01);
    DISABLED_BSS_COLOR_COLLISIONS.fetch_add(1, core::sync::atomic::Ordering::Release);
    true
}

/// Program only the finite HE20 receive-side MMIO leaves reached by the pinned
/// HE capability/operation parsers.
///
/// The register transforms below are exact bounded read/modify/write bodies:
/// they allocate nothing, acquire no lock, call no vendor function, and
/// contain no wait or retry edge. A finite non-disabled RTS threshold is kept
/// fail-closed because the vendor table builder contains a separate floating
/// point loop which this HIL has not recovered yet.
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
#[link_section = ".rwtext.wifi_strict.he_peer"]
pub(crate) unsafe fn program_he20_peer_hardware(
    state: He20PeerState,
) -> Result<(), He20HardwareError> {
    if state.rts_threshold.is_some() {
        return Err(He20HardwareError::UnsupportedRtsThreshold);
    }

    const HE_BSS_COLOR: *mut u32 = 0x2010_4020 as *mut u32;
    const HE_BSS_COLOR_BITMAP_CONTROL: *mut u32 = 0x2010_4048 as *mut u32;
    const HE_ERSU_ACK_RATE: *mut u32 = 0x2010_4404 as *mut u32;
    const HE_ERSU_CONTROL: *mut u32 = 0x2010_4c7c as *mut u32;
    const HE_DEFAULT_PE: *mut u32 = 0x2010_4c80 as *mut u32;
    const HE_PACKET_PADDING: *mut u32 = 0x2010_4c90 as *mut u32;
    const HE_RTS_AC0: *mut u32 = 0x2010_5368 as *mut u32;
    const HE_RTS_AC1: *mut u32 = 0x2010_53e4 as *mut u32;
    const HE_RTS_AC2: *mut u32 = 0x2010_5460 as *mut u32;
    const HE_RTS_AC3: *mut u32 = 0x2010_54dc as *mut u32;

    let color_information = state.bss_color_information;
    let color = u32::from(color_information & 0x3f);
    let partial = color_information & 0x40 != 0;
    let disabled = color_information & 0x80 != 0;
    // `ieee80211_parse_heopr` leaves the register untouched for the all-zero
    // color-information value.
    if color_information & 0xbf != 0 {
        let mut bss_color = HE_BSS_COLOR.read_volatile();
        bss_color &= !(0x0800_0000 | 0x07e0_0000 | 0x1000_0000);
        if !disabled {
            bss_color |= 0x0800_0000 | (color << 21);
        }
        if partial {
            bss_color |= 0x1000_0000;
        }
        HE_BSS_COLOR.write_volatile(bss_color);
    }
    // Collision reporting eventually allocates and sends a vendor robust
    // management action. Until that Rust-owned action path exists, disable
    // its interface-0 producer exactly as the pinned ioctl leaf does and
    // clear the accumulated hardware bitmap. Ordinary BSS-color filtering
    // above remains enabled.
    core::ptr::addr_of_mut!(g_bss_color_collision_detection_enabled).write_volatile(0);
    HE_BSS_COLOR_BITMAP_CONTROL.write_volatile(HE_BSS_COLOR_BITMAP_CONTROL.read_volatile() | 0x01);

    let mut default_pe = HE_DEFAULT_PE.read_volatile();
    default_pe = default_pe & !0x07 | (state.operation_parameters & 0x07);
    HE_DEFAULT_PE.write_volatile(default_pe);

    let padding_us = u32::from(state.packet_padding_eight_us) << 3;
    let repeated_padding = (padding_us & 0x1f)
        | ((padding_us & 0x1f) << 5)
        | ((padding_us & 0x1f) << 10)
        | ((padding_us & 0x1f) << 15)
        | ((padding_us & 0x1f) << 20);
    let packet_padding = HE_PACKET_PADDING.read_volatile() & !0x01ff_ffff;
    HE_PACKET_PADDING.write_volatile(packet_padding | repeated_padding);

    HE_RTS_AC0.write_volatile(HE_RTS_AC0.read_volatile() | 0x0002_0000);
    HE_RTS_AC1.write_volatile(HE_RTS_AC1.read_volatile() | 0x0002_0000);
    HE_RTS_AC2.write_volatile(HE_RTS_AC2.read_volatile() | 0x0002_0000);
    HE_RTS_AC3.write_volatile(HE_RTS_AC3.read_volatile() | 0x0002_0000);

    let mut ersu = HE_ERSU_CONTROL.read_volatile();
    if state.extended_range_single_user {
        ersu &= !0x400;
    } else {
        ersu |= 0x400;
        HE_ERSU_ACK_RATE.write_volatile(0x8080_8080);
    }
    HE_ERSU_CONTROL.write_volatile(ersu);
    Ok(())
}

/// Install the finite interface-0 HE association register state normally
/// written by `hal_he_set_mmss_and_aid`.
///
/// `minimum_mpdu_start_spacing` is the three-bit HT A-MPDU density already
/// negotiated into the static peer. `bssid_index` is zero for an ordinary
/// single-BSSID AP. No vendor rate-control or connection routine is entered.
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-he-association-oracle"
))]
#[link_section = ".rwtext.wifi_strict.he_peer"]
pub(crate) unsafe fn program_he20_association_hardware(
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_index: u8,
) -> Result<(), He20HardwareError> {
    if association_id == 0 || association_id > 0x07ff {
        return Err(He20HardwareError::InvalidAssociationId);
    }

    const HE_STA_CONFIG: *mut u32 = 0x2010_4004 as *mut u32;
    const HE_BROADCAST_RU0: *mut u32 = 0x2010_4038 as *mut u32;
    const HE_BROADCAST_RU1: *mut u32 = 0x2010_403c as *mut u32;

    let mut station = HE_STA_CONFIG.read_volatile();
    station = station & !0x3800_0000 | (u32::from(minimum_mpdu_start_spacing & 0x07) << 27);
    station = station & !0x07ff_0000 | (u32::from(association_id) << 16);
    HE_STA_CONFIG.write_volatile(station);

    let mut broadcast_ru0 = HE_BROADCAST_RU0.read_volatile();
    broadcast_ru0 = broadcast_ru0 & !0x0000_07ff | u32::from(association_id);
    broadcast_ru0 |= 0x0040_0000;
    broadcast_ru0 = broadcast_ru0 & !0x003f_f800 | (u32::from(bssid_index) << 11);
    HE_BROADCAST_RU0.write_volatile(broadcast_ru0);

    let mut broadcast_ru1 = HE_BROADCAST_RU1.read_volatile();
    broadcast_ru1 |= 0x0000_0800;
    broadcast_ru1 &= !0x0000_07ff;
    broadcast_ru1 |= 0x0080_0000;
    broadcast_ru1 &= !0x007f_f000;
    HE_BROADCAST_RU1.write_volatile(broadcast_ru1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_he20_capabilities, parse_he20_operation, parse_he20_peer_state, HeElementError,
        HeMcsNssSupport,
    };

    #[test]
    fn parses_single_stream_mcs9_capability_without_optional_tails() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[20..22].copy_from_slice(&0xfffd_u16.to_le_bytes());
        element[22..24].copy_from_slice(&0xfffd_u16.to_le_bytes());

        let capability = parse_he20_capabilities(&element).unwrap();
        assert_eq!(capability.receive_nss1, HeMcsNssSupport::Mcs0To9);
        assert_eq!(capability.transmit_nss1, HeMcsNssSupport::Mcs0To9);
        assert!(capability.supports_bidirectional_mcs9());
    }

    #[test]
    fn parses_mandatory_operation_prefix_and_masks_bss_color() {
        let element = [255, 7, 36, 0, 0, 0, 0xc5, 0xfd, 0xff];
        let operation = parse_he20_operation(&element).unwrap();
        assert_eq!(operation.bss_color, 5);
        assert_eq!(operation.basic_mcs_nss_map, 0xfffd);
    }

    #[test]
    fn rejects_wrong_extensions_and_incoherent_lengths() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 36]);
        assert_eq!(
            parse_he20_capabilities(&element),
            Err(HeElementError::WrongExtension)
        );
        element[1] = 21;
        assert_eq!(
            parse_he20_capabilities(&element),
            Err(HeElementError::LengthMismatch)
        );
    }

    #[test]
    fn recovers_vendor_ap_he20_peer_state() {
        let capability = [
            0xff, 0x1a, 0x23, 0x05, 0x00, 0x18, 0x12, 0x00, 0x10, 0x22, 0x20, 0x02, 0xc0, 0x0f,
            0x41, 0x95, 0x08, 0x00, 0xcc, 0x00, 0xfa, 0xff, 0xfa, 0xff, 0x19, 0x1c, 0xc7, 0x71,
        ];
        let operation = [0xff, 0x07, 0x24, 0x04, 0x00, 0x01, 0x1b, 0xfc, 0xff];

        let state = parse_he20_peer_state(&capability, &operation).unwrap();
        assert_eq!(state.capability_prefix, capability[..24]);
        assert_eq!(state.max_rate_code, 229);
        assert_eq!(state.packet_padding_eight_us, 2);
        assert_eq!(state.operation_parameters, 0x01_0004);
        assert_eq!(state.bss_color_information, 27);
        assert_eq!(state.basic_mcs_nss_map, 0xfffc);
        assert_eq!(state.rts_threshold, None);
        assert!(state.extended_range_single_user);
    }

    #[test]
    fn peer_state_requires_complete_ppe_prefix() {
        let mut capability = [0_u8; 24];
        capability[..3].copy_from_slice(&[255, 22, 35]);
        capability[15] = 0x80;
        let operation = [255, 7, 36, 0, 0, 0, 0, 0xff, 0xff];
        assert_eq!(
            parse_he20_peer_state(&capability, &operation),
            Err(HeElementError::TooShort)
        );
    }
}
