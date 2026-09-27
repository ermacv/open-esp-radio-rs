use super::*;

fn entry(last: u8, ssid: &[u8]) -> StaPmksa {
    StaPmksa::new(
        [2, 0, 0, 0, 0, last],
        ssid,
        Pmk::from_bytes([last; 32]),
        [last; STA_PMKID_LEN],
    )
    .unwrap()
}

#[test]
fn an_association_resumes_only_for_its_access_point_and_ssid() {
    let mut cache = StaPmksaCache::new();
    cache.insert(entry(1, b"lab"));
    assert_eq!(
        cache
            .resume([2, 0, 0, 0, 0, 1], b"lab")
            .map(StaPmksa::pmkid),
        Some([1; STA_PMKID_LEN])
    );
    assert!(cache.resume([2, 0, 0, 0, 0, 2], b"lab").is_none());
    // The same access point under another SSID flushes its entry.
    assert!(cache.resume([2, 0, 0, 0, 0, 1], b"other").is_none());
    assert!(cache.resume([2, 0, 0, 0, 0, 1], b"lab").is_none());
}

#[test]
fn a_full_cache_drops_its_oldest_entry_and_replacing_refreshes_age() {
    let mut cache = StaPmksaCache::new();
    for last in 0..STA_PMKSA_CAPACITY as u8 {
        cache.insert(entry(last, b"lab"));
    }
    // Re-adding the oldest makes it the newest.
    cache.insert(entry(0, b"lab"));
    cache.insert(entry(100, b"lab"));
    assert!(cache.resume([2, 0, 0, 0, 0, 0], b"lab").is_some());
    assert!(cache.resume([2, 0, 0, 0, 0, 1], b"lab").is_none());
    assert!(cache.resume([2, 0, 0, 0, 0, 100], b"lab").is_some());
}

#[test]
fn removing_forgets_one_access_point() {
    let mut cache = StaPmksaCache::new();
    cache.insert(entry(1, b"lab"));
    cache.insert(entry(2, b"lab"));
    cache.remove([2, 0, 0, 0, 0, 1]);
    assert!(cache.resume([2, 0, 0, 0, 0, 1], b"lab").is_none());
    assert!(cache.resume([2, 0, 0, 0, 0, 2], b"lab").is_some());
}

#[test]
fn an_overlong_ssid_is_not_an_association() {
    assert!(StaPmksa::new([2; 6], &[b'a'; 33], Pmk::from_bytes([0; 32]), [0; 16]).is_none());
}
