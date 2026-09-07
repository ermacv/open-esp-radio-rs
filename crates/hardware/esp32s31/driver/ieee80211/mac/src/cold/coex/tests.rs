use super::MacCoexEvent;

#[test]
fn cold_pti_values_match_the_complete_vendor_table() {
    assert_eq!(MacCoexEvent::Event1.cold_vendor_pti().value(), 5);
    assert_eq!(MacCoexEvent::Event3.cold_vendor_pti().value(), 7);
    assert_eq!(MacCoexEvent::Event10.cold_vendor_pti().value(), 3);
    assert_eq!(MacCoexEvent::Event15.cold_vendor_pti().value(), 1);
}
