//! Pure, explicitly owned PHY parameter transforms.
//!
//! This is the source-only subset of the hybrid compatibility module. It
//! intentionally contains no `phy_param` symbol, ROM pointer cell, callback
//! table, eFuse MMIO read, or C ABI export.

pub(crate) const PHY_PARAM_LEN: usize = 0x1fc;
pub(crate) const PHY_INIT_DATA_LEN: usize = 0x80;
pub(crate) const PHY_CALIBRATION_PAYLOAD_OFFSET: usize = 0x0c;
pub(crate) const PHY_CALIBRATION_CHECKSUM_OFFSET: usize =
    PHY_CALIBRATION_PAYLOAD_OFFSET + PHY_PARAM_LEN;
pub(crate) const PHY_CALIBRATION_PREFIX_LEN: usize = PHY_CALIBRATION_CHECKSUM_OFFSET + 4;

pub(crate) fn apply_init_data(parameter: &mut [u8; PHY_PARAM_LEN], init: &[u8; PHY_INIT_DATA_LEN]) {
    parameter[0x4e] = init[0x00];

    let mut index = 0;
    while index != 18 {
        parameter[0x50 + index] = init[0x02 + index];
        index += 1;
    }

    parameter[0x64] = init[0x18];

    index = 0;
    while index != 14 {
        parameter[0x6e + index] = init[0x19 + index];
        parameter[0x7c + index] = init[0x27 + index];
        parameter[0x8a + index] = init[0x35 + index];
        index += 1;
    }

    index = 0;
    while index != 9 {
        parameter[0x65 + index] = init[0x43 + index];
        index += 1;
    }
}

fn calibration_identity_from_efuse_words(mac_sys0: u32, mac_sys1: u32) -> [u8; 8] {
    [
        (mac_sys1 >> 8) as u8,
        mac_sys1 as u8,
        (mac_sys0 >> 24) as u8,
        (mac_sys0 >> 16) as u8,
        (mac_sys0 >> 8) as u8,
        mac_sys0 as u8,
        (mac_sys1 >> 24) as u8,
        (mac_sys1 >> 16) as u8,
    ]
}

fn read_u32_le(bytes: &[u8; PHY_CALIBRATION_PREFIX_LEN], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn write_u32_le(bytes: &mut [u8; PHY_CALIBRATION_PREFIX_LEN], offset: usize, value: u32) {
    let value = value.to_le_bytes();
    bytes[offset] = value[0];
    bytes[offset + 1] = value[1];
    bytes[offset + 2] = value[2];
    bytes[offset + 3] = value[3];
}

pub(crate) fn calibration_record_check_or_write(
    calibration: &mut [u8; PHY_CALIBRATION_PREFIX_LEN],
    check: bool,
    version: u32,
    mac_sys0: u32,
    mac_sys1: u32,
) -> i32 {
    write_u32_le(calibration, 0, version);
    let identity = calibration_identity_from_efuse_words(mac_sys0, mac_sys1);
    let mut index = 0;
    while index != identity.len() {
        calibration[4 + index] = identity[index];
        index += 1;
    }

    let mut sum = 0_u32;
    let mut offset = 0;
    while offset != PHY_CALIBRATION_CHECKSUM_OFFSET {
        sum = sum.wrapping_add(read_u32_le(calibration, offset));
        offset += 4;
    }
    let checksum = !sum;

    if check {
        i32::from(checksum != read_u32_le(calibration, PHY_CALIBRATION_CHECKSUM_OFFSET))
    } else {
        write_u32_le(calibration, PHY_CALIBRATION_CHECKSUM_OFFSET, checksum);
        0
    }
}

pub(crate) const fn saturate_phy_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

pub(crate) fn apply_rc_calibration_result(parameter: &mut [u8; PHY_PARAM_LEN], result: u8) {
    const NUMERATOR_SCALE: i32 = 82;
    const AUXILIARY_NUMERATOR_SCALE: i32 = 0x334;
    const AUXILIARY_DIVISOR_SCALE: i32 = 104;
    const UPPER_BOUNDS: [u8; 4] = [0x28, 0x14, 0x1e, 0x14];
    const PRIMARY_DIVISORS: [u8; 2] = [0x14, 0x28];
    const AUXILIARY_DIVISORS: [u8; 4] = [0x24, 0x28, 0x16, 0x20];

    parameter[0xe8] = result;
    let bounded_result = if result > 45 { 50 } else { result };
    let base = bounded_result as i32 + 56;
    let primary_numerator = base * NUMERATOR_SCALE;

    let mut index = 0;
    while index != PRIMARY_DIVISORS.len() {
        let divisor = PRIMARY_DIVISORS[index] as i32 * 10;
        let value = primary_numerator / divisor - 8;
        parameter[0xe9 + index] = saturate_phy_value(value, UPPER_BOUNDS[index], 2);
        index += 1;
    }

    let auxiliary_numerator = base * AUXILIARY_NUMERATOR_SCALE;
    index = 0;
    while index != AUXILIARY_DIVISORS.len() {
        let divisor = AUXILIARY_DIVISORS[index] as i32 * AUXILIARY_DIVISOR_SCALE;
        let value = auxiliary_numerator / divisor - 8;
        parameter[0xed + index] = saturate_phy_value(value, UPPER_BOUNDS[2 + (index & 1)], 0);
        index += 1;
    }

    let mut flags = u32::from_le_bytes([
        parameter[0xa4],
        parameter[0xa5],
        parameter[0xa6],
        parameter[0xa7],
    ]);
    flags |= 1 << 23;
    parameter[0xa4..0xa8].copy_from_slice(&flags.to_le_bytes());
}

pub(crate) const fn xtal_parameter_code(frequency_mhz: u32) -> u8 {
    match frequency_mhz {
        26 => 1,
        32 => 2,
        _ => 0,
    }
}
