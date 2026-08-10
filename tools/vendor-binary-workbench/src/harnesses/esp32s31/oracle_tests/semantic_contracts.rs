use super::super::*;
use crate::harnesses::esp32s31::RISCV_HARNESS;

#[test]
fn registered_phy_contract_composes_pinned_i2c_polling_summaries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();

    for symbol in ["phy_chip_i2c_readReg", "phy_chip_i2c_writeReg"] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                .unwrap();
        assert!(generated.source.contains("// Poll until"));
        assert_generated_reference_compiles(symbol, &generated.source);
    }
}

#[test]
fn registered_phy_contract_composes_exact_rom_wide_division() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();

    let trace = catalog.trace(None, "phy_rfpll_set_freq", &svd).unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    assert_eq!(
        trace
            .reference_dependencies
            .iter()
            .filter(|dependency| dependency.as_str() == "__divdi3")
            .count(),
        8,
        "the four structured paths each contain the exact two-call division chain"
    );
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert!(generated.source.contains("riscv_div_i64_words"));
    assert!(generated.source.contains("call_result0_high"));
    assert_generated_reference_compiles("phy_rfpll_set_freq", &generated.source);

    let divdi3 = catalog
        .symbols_by_address
        .get_mut(&crate::harnesses::esp32s31::wide_signed_divide_target_address())
        .expect("pinned ROM must contain __divdi3");
    divdi3.bytes[0] ^= 1;
    let changed = catalog.trace(None, "phy_rfpll_set_freq", &svd).unwrap();
    assert!(!changed.is_reference_eligible());
    assert!(
        changed
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("__divdi3")),
        "{changed:#?}"
    );
}

#[test]
fn registered_phy_contract_composes_the_bounded_rfpll_poll() {
    const PHY_WAIT_RFPLL_CAL_END_ADDRESS: u32 = 0x2f82_5874;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();

    let trace = catalog.trace(None, "phy_wait_rfpll_cal_end", &svd).unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("for bounded_poll_attempt0 in 0..100_u16")
    );
    assert!(
        generated
            .source
            .contains("platform.ets_printf(0x2f84d9cc_u32)")
    );
    assert_generated_reference_compiles("phy_wait_rfpll_cal_end", &generated.source);

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_WAIT_RFPLL_CAL_END_ADDRESS))
        .expect("pinned ROM must contain phy_wait_rfpll_cal_end");
    symbol.bytes[0] ^= 1;
    let changed = catalog.trace(None, "phy_wait_rfpll_cal_end", &svd).unwrap();
    assert!(!changed.is_reference_eligible());
    assert!(
        changed
            .reference_failure_reasons()
            .iter()
            .any(|reason| reason.contains("branch exploration did not cover both outcomes")),
        "{changed:#?}"
    );
}

#[test]
fn registered_phy_contract_composes_the_rfpll_cap_search() {
    const PHY_RFPLL_CAP_INIT_CAL_ADDRESS: u32 = 0x2f82_5ada;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();
    let calibration_tests = r#"
struct CalibrationIo {
    initial: u16,
    statuses: Vec<u8>,
    status_index: usize,
    read_register: u8,
    read_phase: u8,
}

impl CalibrationIo {
    fn new(initial: u16, statuses: Vec<u8>) -> Self {
        Self { initial, statuses, status_index: 0, read_register: 0, read_phase: 0 }
    }

    fn read_data(&mut self) -> u8 {
        match self.read_register {
            0x05 => self.initial as u8,
            0x07 => (((self.initial >> 8) & 1) as u8) << 2,
            0x0c => {
                let status = self.statuses.get(self.status_index).copied().unwrap_or(1);
                self.status_index += 1;
                status << 2
            }
            _ => 0,
        }
    }
}

impl ReferenceIo for CalibrationIo {
    fn read(&mut self, _width: u8, address: u32) -> u32 {
        if address == 0x2010f820 { return 0; }
        if !matches!(address, 0x2010f800 | 0x2010f804) { return 0; }
        match self.read_phase {
            2 => { self.read_phase = 1; 0 }
            1 => { self.read_phase = 0; u32::from(self.read_data()) << 16 }
            _ => 0,
        }
    }

    fn write(&mut self, _width: u8, address: u32, value: u32) {
        if matches!(address, 0x2010f800 | 0x2010f804)
            && value & 0x07000000 == 0x04000000
        {
            self.read_register = ((value >> 8) & 0xff) as u8;
            self.read_phase = 2;
        }
    }

    fn delay_micros(&mut self, _micros: u32) {}
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

struct CalibrationMemory;
impl ReferenceMemory for CalibrationMemory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0 }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
}

struct CalibrationPlatform;
impl ReferencePlatform for CalibrationPlatform {
    fn external_table_version(&mut self, _table: &str) -> u32 { 9 }
    fn external_table_magic(&mut self, _table: &str) -> u32 { 0xdeadbeaf }
    fn external_table_size(&mut self, _table: &str) -> u32 { 512 }
    fn external_call(&mut self, _table: &str, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn direct_external_call(&mut self, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn diagnostic_call(&mut self, _function: &str, _arguments: &[u32]) {}
}

fn run_calibration(initial: u16, statuses: Vec<u8>) -> (u32, usize) {
    let mut io = CalibrationIo::new(initial, statuses);
    let mut memory = CalibrationMemory;
    let mut platform = CalibrationPlatform;
    let outcome = open_phy_reference_phy_rfpll_cap_init_cal(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers: [0; 8], stack: [0; 8] },
    );
    (outcome.exit_a0.unwrap(), io.status_index)
}

#[test]
fn all_candidates_are_averaged_across_both_directions() {
    assert_eq!(run_calibration(16, vec![0; 20]), (0x00100010, 20));
}

#[test]
fn no_accepted_candidate_preserves_the_initial_cap() {
    assert_eq!(run_calibration(16, vec![1; 20]), (0x00100010, 20));
}

#[test]
fn accepted_window_stops_after_its_first_rejection() {
    assert_eq!(run_calibration(16, vec![1, 0, 0, 1, 1]), (0x0010000e, 5));
}
"#;

    for symbol in ["phy_rfpll_cap_init_cal", "phy_set_rfpll_freq"] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        let generated =
            generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                .unwrap();
        assert!(
            generated
                .source
                .contains("for calibration_direction0 in 0..2_u8")
        );
        assert!(
            generated
                .source
                .contains("for calibration_step0 in 0..10_u16")
        );
        assert!(generated.source.contains("calibration_sum0.wrapping_add"));
        assert_generated_reference_compiles(symbol, &generated.source);
        if symbol == "phy_rfpll_cap_init_cal" {
            assert_generated_reference_tests_run(symbol, &generated.source, calibration_tests);
        }
    }

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_RFPLL_CAP_INIT_CAL_ADDRESS))
        .expect("pinned ROM must contain phy_rfpll_cap_init_cal");
    symbol.bytes[0] ^= 1;
    if let Ok(changed) = catalog.trace(None, "phy_rfpll_cap_init_cal", &svd) {
        assert!(!changed.is_reference_eligible(), "{changed:#?}");
    }
}

#[test]
fn registered_phy_contract_scopes_the_rf_frequency_scratch() {
    const PHY_SET_RF_FREQ_OFFSET_ADDRESS: u32 = 0x2f82_5c10;
    const SCRATCH_TESTS: &str = r#"
struct BackingMemory;
impl ReferenceMemory for BackingMemory {
    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 { 0 }
    fn read(&mut self, _width: u8, _address: u32) -> u32 { 0xaabbccdd }
    fn write(&mut self, _width: u8, _address: u32, _value: u32) {}
}

#[test]
fn scratch_round_trips_little_endian_bytes_and_delegates_disjoint_reads() {
    let mut backing = BackingMemory;
    let mut scratch = ReferenceScratchMemory::new(&mut backing, 0xfffe0000, 5);
    scratch.write(32, 0xfffe0000, 0x44332211);
    scratch.write(8, 0xfffe0004, 0x55);
    assert_eq!(scratch.read(32, 0xfffe0000), 0x44332211);
    assert_eq!(scratch.read(8, 0xfffe0004), 0x55);
    assert_eq!(scratch.read(32, 0x10000000), 0xaabbccdd);
}

#[test]
#[should_panic(expected = "read from uninitialized reference scratch")]
fn scratch_rejects_uninitialized_reads() {
    let mut backing = BackingMemory;
    let mut scratch = ReferenceScratchMemory::new(&mut backing, 0xfffe0000, 5);
    let _ = scratch.read(8, 0xfffe0000);
}

#[test]
#[should_panic(expected = "partially overlaps private scratch")]
fn scratch_rejects_partial_overlap() {
    let mut backing = BackingMemory;
    let mut scratch = ReferenceScratchMemory::new(&mut backing, 0xfffe0000, 5);
    let _ = scratch.read(16, 0xfffdffff);
}
"#;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();

    for symbol in [
        "phy_set_rf_freq_offset",
        "phy_set_channel_rfpll_freq",
        "phy_set_freq",
        "phy_chip_set_chan_ana",
        "phy_dcode_cal_init",
    ] {
        let trace = catalog.trace(None, symbol, &svd).unwrap();
        assert!(trace.is_reference_eligible(), "{symbol}: {trace:#?}");
        if matches!(symbol, "phy_set_rf_freq_offset" | "phy_chip_set_chan_ana") {
            let generated =
                generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[])
                    .unwrap();
            assert!(generated.source.contains("ReferenceScratchMemory::new"));
            assert_generated_reference_compiles(symbol, &generated.source);
            if symbol == "phy_set_rf_freq_offset" {
                assert_generated_reference_tests_run(symbol, &generated.source, SCRATCH_TESTS);
            }
        }
    }

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_SET_RF_FREQ_OFFSET_ADDRESS))
        .expect("pinned ROM must contain phy_set_rf_freq_offset");
    symbol.bytes[0] ^= 1;
    if let Ok(changed) = catalog.trace(None, "phy_set_rf_freq_offset", &svd) {
        assert!(!changed.is_reference_eligible(), "{changed:#?}");
    }
}

#[test]
fn registered_phy_contract_models_the_live_iq_estimator_poll() {
    const PHY_IQ_EST_ENABLE_ADDRESS: u32 = 0x2f82_89d4;
    const IQ_ESTIMATOR_TESTS: &str = r#"
struct EstimatorIo {
    done: Vec<u32>,
    statuses: Vec<u32>,
    done_index: usize,
    status_index: usize,
    writes: Vec<(u32, u32)>,
    delays: Vec<u32>,
}

impl EstimatorIo {
    fn new(done: Vec<u32>, statuses: Vec<u32>) -> Self {
        Self {
            done,
            statuses,
            done_index: 0,
            status_index: 0,
            writes: Vec::new(),
            delays: Vec::new(),
        }
    }
}

impl ReferenceIo for EstimatorIo {
    fn read(&mut self, width: u8, address: u32) -> u32 {
        assert_eq!(width, 32);
        match address {
            0x2010044c | 0x20100450 => 0,
            0x2010047c => {
                let value = self.done.get(self.done_index).copied().unwrap_or(0x00010000);
                self.done_index += 1;
                value
            }
            0x201008d0 => {
                let value = self.statuses.get(self.status_index).copied().unwrap_or(0);
                self.status_index += 1;
                value
            }
            _ => panic!("unexpected MMIO read at {address:#010x}"),
        }
    }

    fn write(&mut self, width: u8, address: u32, value: u32) {
        assert_eq!(width, 32);
        self.writes.push((address, value));
    }

    fn delay_micros(&mut self, micros: u32) { self.delays.push(micros); }
    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {}
}

struct EstimatorMemory {
    base: u32,
    counter: u16,
}

impl ReferenceMemory for EstimatorMemory {
    fn symbol_address(&mut self, member: Option<&str>, symbol: &str) -> u32 {
        assert_eq!(member, None);
        assert_eq!(symbol, "phy_param");
        self.base
    }

    fn read(&mut self, width: u8, address: u32) -> u32 {
        assert_eq!((width, address), (16, self.base + 0x1ac));
        u32::from(self.counter)
    }

    fn write(&mut self, width: u8, address: u32, value: u32) {
        assert_eq!((width, address), (16, self.base + 0x1ac));
        self.counter = value as u16;
    }
}

struct EstimatorPlatform;
impl ReferencePlatform for EstimatorPlatform {
    fn external_table_version(&mut self, _table: &str) -> u32 { 9 }
    fn external_table_magic(&mut self, _table: &str) -> u32 { 0xdeadbeaf }
    fn external_table_size(&mut self, _table: &str) -> u32 { 512 }
    fn external_call(&mut self, _table: &str, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn direct_external_call(&mut self, _function: &str, _arguments: &[u32]) -> u32 { 0 }
    fn diagnostic_call(&mut self, _function: &str, _arguments: &[u32]) {}
}

fn run_estimator(done: Vec<u32>, statuses: Vec<u32>) -> (EstimatorIo, EstimatorMemory) {
    let mut io = EstimatorIo::new(done, statuses);
    let mut memory = EstimatorMemory { base: 0x3fcd0000, counter: 0xffff };
    let mut platform = EstimatorPlatform;
    let mut registers = [0; 8];
    registers[1] = 0x12345;
    let outcome = open_phy_reference_phy_iq_est_enable(
        &mut io,
        &mut memory,
        &mut platform,
        Rv32ReferenceArguments { registers, stack: [0; 8] },
    );
    assert_eq!(outcome.exit_a0, None);
    (io, memory)
}

#[test]
fn immediate_done_does_not_sample_activity_status() {
    let (io, memory) = run_estimator(vec![0x00010000], vec![]);
    assert_eq!(memory.counter, 0);
    assert_eq!((io.done_index, io.status_index), (1, 0));
    assert_eq!(io.delays, [1]);
    assert_eq!(
        io.writes,
        [
            (0x2010044c, 0x04000000),
            (0x20100450, 0x00100000),
            (0x20100450, 0x00008d14),
            (0x20100450, 0x00000001),
            (0x20100450, 0x00000002),
        ]
    );
}

#[test]
fn live_reads_increment_only_on_active_not_done_iterations() {
    let (io, memory) = run_estimator(
        vec![0, 0, 0x00010000],
        vec![0, 0x00100000],
    );
    assert_eq!(memory.counter, 1);
    assert_eq!((io.done_index, io.status_index), (3, 2));
}
"#;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workbench facade remains under tools");
    let artifact = private_input("OPEN_ESP_RADIO_ESP32S31_ROM_ELF").unwrap_or_default();
    let companion = root.join(
        "verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf",
    );
    if !artifact.exists() || !companion.exists() {
        eprintln!("private linked PHY fixtures are not installed; integration test skipped");
        return;
    }
    let svd = MmioMap::load_all(&[
        root.join("svd/esp32s31-radio.svd"),
        root.join("svd/esp32s31-platform-radio-deps.svd"),
    ])
    .unwrap();
    let mut catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &[companion],
        &RISCV_HARNESS,
        entry_contract::PHY_REGISTERED,
    )
    .unwrap();

    let trace = catalog.trace(None, "phy_iq_est_enable", &svd).unwrap();
    assert!(trace.is_reference_eligible(), "{trace:#?}");
    let generated =
        generate_reference(&trace, "esp32s31_rev0_rom.elf", "fixture-sha256", None, &[]).unwrap();
    assert!(generated.source.contains("Poll a complete composed flow"));
    assert!(
        generated
            .source
            .contains("memory.symbol_address(None, \"phy_param\")")
    );
    assert!(generated.source.contains("io.read(32, 0x2010047c_u32)"));
    assert!(generated.source.contains("io.read(32, 0x201008d0_u32)"));
    assert_generated_reference_compiles("phy_iq_est_enable", &generated.source);
    assert_generated_reference_tests_run(
        "phy_iq_est_enable",
        &generated.source,
        IQ_ESTIMATOR_TESTS,
    );

    let symbol = catalog
        .symbols
        .iter_mut()
        .find(|symbol| symbol.address == u64::from(PHY_IQ_EST_ENABLE_ADDRESS))
        .expect("pinned ROM must contain phy_iq_est_enable");
    symbol.bytes[0] ^= 1;
    if let Ok(changed) = catalog.trace(None, "phy_iq_est_enable", &svd) {
        assert!(!changed.is_reference_eligible(), "{changed:#?}");
    }
}
