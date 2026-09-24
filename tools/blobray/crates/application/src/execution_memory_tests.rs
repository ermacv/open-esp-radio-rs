use super::*;
const ORDER: ExecutionOrdering = ExecutionOrdering {
    acquire: true,
    release: true,
};
fn session(memory: &WorkingMemory, fill: Option<u8>) -> Session<'_> {
    let mut s = Session::new(memory, 4, &mut || Ok(())).unwrap();
    s.region(
        Mapping {
            address: 0x1000,
            length: 8,
            flags: 6,
            kind: RegionKind::Ram,
        },
        fill,
        &[],
        &mut || Ok(()),
    )
    .unwrap();
    s
}
#[test]
fn reservations_follow_physical_writes_and_each_sc_consumes_them() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut s = session(&memory, Some(0));
    let mut c = || Ok(());
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(s.load_reserved(0x1000, ORDER, &mut c).unwrap(), Some(0));
    assert!(s.write(0x1004, 4, 8, &mut c).unwrap()); // disjoint store retains reservation
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(true)
    );
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut c).unwrap(),
        Some(7)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert!(s.write(0x1001, 1, 0, &mut c).unwrap()); // even an unchanged byte invalidates
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    s.load_reserved(0x1004, ORDER, &mut c).unwrap();
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.store_conditional(0x1004, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(s.store_conditional(0x5000, 9, ORDER, &mut c).unwrap(), None);
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(s.load_reserved(0x1001, ORDER, &mut c).unwrap(), None);
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert!(s.events.is_empty());
    drop(s);
    assert_eq!(memory.observation().reserved_bytes, 0);
}

#[test]
fn atomic_update_is_once_after_validation_and_invalidates_only_overlap() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut s = session(&memory, None);
    let mut c = || Ok(());
    let mut calls = 0;
    for address in [0x1000, 0x1001, 0x5000] {
        assert_eq!(
            s.modify_word(
                address,
                ORDER,
                &mut |_| {
                    calls += 1;
                    7
                },
                &mut c
            )
            .unwrap(),
            None
        );
    }
    assert_eq!(calls, 0);
    assert!(s.write(0x1000, 4, 3, &mut c).unwrap());
    let error = s
        .modify_word(
            0x1000,
            ORDER,
            &mut |_| {
                calls += 1;
                99
            },
            &mut || {
                Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "fixture exhausted work",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert_eq!(calls, 0);
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut c).unwrap(),
        Some(3)
    );
    assert!(s.write(0x1004, 4, 4, &mut c).unwrap());
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(
        s.modify_word(
            0x1004,
            ORDER,
            &mut |n| {
                calls += 1;
                n + 1
            },
            &mut c
        )
        .unwrap(),
        Some(4)
    );
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(true)
    );
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(
        s.modify_word(
            0x1000,
            ORDER,
            &mut |n| {
                calls += 1;
                n + 1
            },
            &mut c
        )
        .unwrap(),
        Some(7)
    );
    assert_eq!(
        s.store_conditional(0x1000, 9, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.read(0x1000, 4, MemoryAccess::Read, &mut c).unwrap(),
        Some(8)
    );
    assert_eq!(calls, 2);
    let mut other = session(&memory, Some(0));
    s.load_reserved(0x1000, ORDER, &mut c).unwrap();
    assert_eq!(
        other.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(false)
    );
    assert_eq!(
        s.store_conditional(0x1000, 7, ORDER, &mut c).unwrap(),
        Some(true)
    );
    assert!(s.events.is_empty());
}
