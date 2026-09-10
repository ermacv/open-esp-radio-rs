//! Actual vendor state serialization and gain consumption, not RF equivalence.
use super::*;
use open_radio_vendor_models_esp32s31::execution_model::MemoryRange;

const CACHE: u32 = 0x3ffe_1000;
const OUTPUT: u32 = 0x3ffe_2000;
const INIT: u32 = 0x3ffe_3000;
// Private ABI scenario boundaries, not production storage or PAC assertions.
const STATE_LEN: usize = 516;
const CACHE_PREFIX: usize = 12;
const CACHE_LEN: usize = 532;
const ADJUSTMENT: usize = 434;

fn bytes(
    session: &execution::ExecutionSession,
    image: &ExecutableImage,
    address: u32,
    length: usize,
) -> Vec<u8> {
    (0..length)
        .map(|i| {
            session
                .byte(image, address + i as u32)
                .expect("INCOMPLETE retained byte")
        })
        .collect()
}

fn put(scenario: &mut Scenario, address: u32, bytes: impl IntoIterator<Item = u8>) {
    scenario.memory_initial.extend(
        bytes
            .into_iter()
            .enumerate()
            .map(|(i, byte)| (address + i as u32, byte)),
    );
}

fn run(
    session: &mut execution::ExecutionSession,
    image: &ExecutableImage,
    entry: &str,
    scenario: Scenario,
) {
    let result = session
        .execute(
            image,
            &MmioMap {
                registers: vec![],
                regions: vec![],
            },
            entry,
            scenario,
        )
        .unwrap_or_else(|error| panic!("INCOMPLETE {entry}: {error}"));
    assert!(result.events.is_empty(), "{entry} touched hardware");
}

#[test]
#[ignore = "requires authenticated current archive and ROM; vendor characterization, not cache validation or production equivalence; run through blobray-run"]
fn current_gain_adjustment_survives_calibration_storage_and_init_parameters() {
    let archive = input(
        "OER_PHY_ARCHIVE",
        Some("d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"),
    );
    let rom = input(
        "OER_PHY_ROM",
        Some("d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"),
    );
    let image = ExecutableImage::load_entry_with_roots(
        &archive,
        "phy_rf_cal_data_backup_new",
        &[
            "phy_rf_cal_data_recovery_new",
            "phy_wifi_get_tx_tab_new",
            "register_chipv7_phy_init_param",
        ],
        Some(&rom),
    )
    .expect("load actual calibration storage and gain bodies");
    let param = image.symbol_address("phy_param").unwrap();
    assert_eq!(
        execution::ExecutionSession::default().byte(&image, param + ADJUSTMENT as u32),
        Some(0),
        "archive startup value"
    );

    for adjustment in [0_u8, 1, 31, 127, 128, 255] {
        for fill in [0x5a, 0xa5] {
            let mut session = execution::ExecutionSession::default();
            let mut state: Vec<_> = (0..STATE_LEN)
                .map(|i| (i as u8).wrapping_mul(37).wrapping_add(fill))
                .collect();
            state[ADJUSTMENT] = adjustment;
            let mut backup = Scenario {
                arguments: vec![CACHE],
                private_stack_fill: Some(fill),
                persistent_memory: vec![MemoryRange {
                    start: CACHE,
                    length: CACHE_LEN as u32,
                }],
                ..Default::default()
            };
            put(&mut backup, param, state.iter().copied());
            put(&mut backup, CACHE, [fill; CACHE_LEN]);
            run(&mut session, &image, "phy_rf_cal_data_backup_new", backup);
            let saved = bytes(&session, &image, CACHE, CACHE_LEN);
            assert_eq!(&saved[..CACHE_PREFIX], &[fill; CACHE_PREFIX]);
            assert_eq!(&saved[CACHE_PREFIX..CACHE_PREFIX + STATE_LEN], &state);
            assert_eq!(&saved[CACHE_PREFIX + STATE_LEN..], &[fill; 4]);

            let mut recovery = Scenario {
                arguments: vec![CACHE],
                private_stack_fill: Some(fill),
                ..Default::default()
            };
            // Destroy the complete live state; the next call must recover it
            // from the bytes actually produced by backup in the same session.
            put(&mut recovery, param, state.iter().map(|byte| !byte));
            run(
                &mut session,
                &image,
                "phy_rf_cal_data_recovery_new",
                recovery,
            );
            assert_eq!(bytes(&session, &image, param, STATE_LEN), state);

            let mut init = Scenario {
                arguments: vec![INIT],
                private_stack_fill: Some(fill),
                ..Default::default()
            };
            put(&mut init, INIT, [fill; 128]);
            run(&mut session, &image, "register_chipv7_phy_init_param", init);
            assert_eq!(
                session.byte(&image, param + ADJUSTMENT as u32),
                Some(adjustment)
            );

            let mut gain = Scenario {
                arguments: vec![13, OUTPUT, OUTPUT + 32, OUTPUT + 96, 0],
                private_stack_fill: Some(fill),
                persistent_memory: vec![MemoryRange {
                    start: OUTPUT,
                    length: 160,
                }],
                ..Default::default()
            };
            put(&mut gain, OUTPUT, [fill; 160]);
            // Isolate this restored input from unrelated synthetic calibration
            // coefficients. Never seed the adjustment again after recovery.
            put(&mut gain, param + 241, [0; 7]);
            put(&mut gain, param + 291, [0]);
            run(
                &mut session,
                &image,
                "phy_wifi_get_tx_tab_new",
                gain.clone(),
            );
            let restored_output = bytes(&session, &image, OUTPUT, 160);
            put(&mut gain, param + ADJUSTMENT as u32, [0]);
            put(&mut gain, param + 291, [adjustment]);
            run(&mut session, &image, "phy_wifi_get_tx_tab_new", gain);
            assert_eq!(restored_output, bytes(&session, &image, OUTPUT, 160));
            println!(
                "CHARACTERIZED calibration storage → init parameters → gain adjustment={adjustment} fill={fill:x}"
            );
        }
    }
}
