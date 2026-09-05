use super::*;

#[test]
fn ethernet_builder_owns_complete_eapol_frame() {
    let mut m4 = Wpa2TxFrame::<128>::message4([1; 6], 9).unwrap();
    m4.set_mic(&[0xa5; 16]);
    let ethernet = Wpa2EthernetFrame::<160>::build([2; 6], &m4).unwrap();
    assert_eq!(&ethernet.as_bytes()[..6], &[1; 6]);
    assert_eq!(&ethernet.as_bytes()[6..12], &[2; 6]);
    assert_eq!(&ethernet.as_bytes()[12..14], &EAPOL_ETHERTYPE);
    assert_eq!(&ethernet.as_bytes()[14 + 81..14 + 97], &[0xa5; 16]);
}
