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

#[test]
fn an_owned_cache_outlives_connections_without_shared_state() {
    let mut cache = StaPmksaCache::new();
    let bssid = [2, 1, 2, 3, 4, 5];
    cache.insert(StaPmksa::new(bssid, b"lab", Pmk::from_bytes([7; 32]), [9; 16]).unwrap());
    // A connection borrows the owner, then returns it for the next one.
    for _ in 0..2 {
        let cached = cache.resume(bssid, b"lab").unwrap();
        assert_eq!(cached.pmkid(), [9; 16]);
        let binding = cached.pmk().bind_association_security_ies(b"association");
        assert!(binding.matches(&Pmk::from_bytes([7; 32]), b"association"));
    }
    cache.remove(bssid);
    assert!(cache.resume(bssid, b"lab").is_none());
}

#[test]
fn a_changed_ssid_compacts_the_cache_without_changing_other_entries_ages() {
    let mut cache = StaPmksaCache::new();
    for last in 0..STA_PMKSA_CAPACITY as u8 {
        cache.insert(entry(last, b"lab"));
    }
    assert!(cache.resume([2, 0, 0, 0, 0, 5], b"other").is_none());
    cache.insert(entry(100, b"lab"));
    assert!(cache.resume([2, 0, 0, 0, 0, 0], b"lab").is_some());
    cache.insert(entry(101, b"lab"));
    assert!(cache.resume([2, 0, 0, 0, 0, 0], b"lab").is_none());
    for last in [1, 2, 3, 4, 6, 7, 8, 9, 100, 101] {
        assert!(cache.resume([2, 0, 0, 0, 0, last], b"lab").is_some());
    }
}
