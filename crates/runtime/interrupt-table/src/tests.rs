use crate::{Error, Matrix, disable, enable, install, verify};

crate::__fake_matrix!();

#[test]
fn install_silences_the_current_core_s_sources_and_checks_their_slots() {
    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, __OER_INTERRUPT_TABLE).unwrap();
    // The timer is core zero's entry; core one's radio is not core zero's to
    // touch.
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);
    assert_eq!(matrix.routed(Core::One, Source::Radio), Some(Level::Two));

    let mut matrix = FakeMatrix::new(Core::Zero);
    matrix.slots[Source::Timer as usize] = 0x1234;
    assert_eq!(
        install(&mut matrix, __OER_INTERRUPT_TABLE),
        Err(Error::ForeignHandler {
            source: Source::Timer
        })
    );
}

#[test]
fn a_token_routes_its_source_to_its_level_on_its_core_alone() {
    let tokens = Interrupts::take().unwrap();
    assert!(Interrupts::take().is_none());

    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, __OER_INTERRUPT_TABLE).unwrap();
    enable(&mut matrix, __OER_INTERRUPT_TABLE, &tokens.timer).unwrap();
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), Some(Level::One));
    verify(&matrix, __OER_INTERRUPT_TABLE).unwrap();
    disable(&mut matrix, &tokens.timer);
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);

    // The radio belongs to core one.
    assert_eq!(
        enable(&mut matrix, __OER_INTERRUPT_TABLE, &tokens.radio),
        Err(Error::WrongCore {
            source: Source::Radio,
            core: Core::One,
            current: Core::Zero
        })
    );
    assert_eq!(matrix.routed(Core::Zero, Source::Radio), Some(Level::Two));

    // A token whose source the table does not list enables nothing.
    assert_eq!(
        enable(&mut matrix, &[], &tokens.timer),
        Err(Error::NotInTable {
            source: Source::Timer
        })
    );
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);
}

#[test]
fn verify_rejects_a_source_routed_to_another_level() {
    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, __OER_INTERRUPT_TABLE).unwrap();
    matrix.route(Source::Timer, Level::Two);
    assert_eq!(
        verify(&matrix, __OER_INTERRUPT_TABLE),
        Err(Error::WrongLevel {
            source: Source::Timer,
            level: Level::Two
        })
    );
}

#[test]
fn a_table_lists_each_source_once() {
    let matrix = FakeMatrix::new(Core::Zero);
    let twice = [
        Some(crate::Binding {
            source: Source::Timer,
            level: Level::One,
            core: Core::Zero,
            handler: Timer,
        }),
        Some(crate::Binding {
            source: Source::Timer,
            level: Level::One,
            core: Core::Zero,
            handler: Timer,
        }),
    ];
    assert_eq!(
        verify(&matrix, &twice),
        Err(Error::Duplicate {
            source: Source::Timer
        })
    );
}

#[test]
fn an_entry_its_cfg_leaves_out_has_no_binding() {
    assert_eq!(__OER_INTERRUPT_TABLE.len(), 3);
    assert!(__OER_INTERRUPT_TABLE[2].is_none());
}
