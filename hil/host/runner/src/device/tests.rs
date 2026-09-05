use super::*;

#[test]
fn ota0_selector_uses_valid_idf_entry() {
    let image = ota0_selector_image();
    assert_eq!(u32::from_le_bytes(image[0..4].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(image[24..28].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(image[28..32].try_into().unwrap()),
        crc32_idf(&1_u32.to_le_bytes())
    );
    assert!(image[32..].iter().all(|byte| *byte == 0xff));
}
