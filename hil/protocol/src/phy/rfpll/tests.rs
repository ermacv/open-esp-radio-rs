use super::*;

fn detail(delta: i16) -> RfpllEvidence {
    RfpllEvidence {
        temperature: 40,
        sample_age_micros: None,
        reference_before: 20,
        reference_after: 40,
        threshold: 15,
        channel: 13,
        correction: Some(RfpllCorrectionEvidence {
            initial_cap: 100,
            selected_cap: (100_i16 + delta) as u16,
            accepted_samples: 2,
            entries_updated: if delta == 0 { 0 } else { 85 },
            restored_frequency_index: (delta != 0).then_some(13),
        }),
    }
}

#[test]
fn thermal_boundary_distinguishes_skip_from_zero_correction() {
    for (temperature, reference) in [
        (14, 0),
        (15, 0),
        (-14, 0),
        (-15, 0),
        (i16::MAX, i16::MIN),
        (i16::MIN, i16::MAX),
    ] {
        let due = (i32::from(temperature) - i32::from(reference)).unsigned_abs() >= 15;
        let ran = RfpllEvidence {
            temperature,
            reference_before: reference,
            reference_after: temperature,
            ..detail(0)
        };
        let skipped = RfpllEvidence {
            correction: None,
            reference_after: reference,
            ..ran
        };
        assert_eq!(ran.is_valid(), due);
        assert_eq!(skipped.is_valid(), !due);
    }
    let ran = RfpllEvidence {
        temperature: 20,
        reference_after: 20,
        threshold: 0,
        ..detail(0)
    };
    assert!(ran.is_valid());
    assert!(
        !RfpllEvidence {
            correction: None,
            ..ran
        }
        .is_valid()
    );
}

#[test]
fn correction_requires_completed_memory_work_exactly_for_nonzero_delta() {
    for delta in [0, 5, -5] {
        let report = detail(delta);
        assert!(report.is_valid());
        let correction = report.correction.unwrap();
        assert_eq!(correction.delta(), delta);
        for broken in [
            RfpllCorrectionEvidence {
                accepted_samples: 21,
                ..correction
            },
            RfpllCorrectionEvidence {
                entries_updated: if delta == 0 { 85 } else { 0 },
                ..correction
            },
            RfpllCorrectionEvidence {
                restored_frequency_index: if delta == 0 { Some(13) } else { None },
                ..correction
            },
        ] {
            assert!(
                !RfpllEvidence {
                    correction: Some(broken),
                    ..report
                }
                .is_valid()
            );
        }
        assert!(
            !RfpllEvidence {
                reference_after: 20,
                ..report
            }
            .is_valid()
        );
    }
    let report = detail(0);
    assert!(
        RfpllEvidence {
            correction: Some(RfpllCorrectionEvidence {
                accepted_samples: 0,
                ..report.correction.unwrap()
            }),
            ..report
        }
        .is_valid()
    );
    let report = detail(5);
    assert!(
        !RfpllEvidence {
            correction: Some(RfpllCorrectionEvidence {
                accepted_samples: 0,
                ..report.correction.unwrap()
            }),
            ..report
        }
        .is_valid()
    );
}
