//! Characterize the linked vendor COEX/ROM protection path, including actual
//! callback installation. RAM/MMIO are modeled: this is not RF grant evidence.

use open_radio_vendor_models_esp32s31::execution_model::{DeviceModelSpec, MemoryRange};
use open_radio_vendor_models_esp32s31::{
    MmioMap, MmioRegion,
    execution::{
        ExecutableImage, ExecutionEvent, ExecutionResult, ExecutionSession, ModeledAllocation,
        ModeledCallResponse, Scenario,
    },
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};

// Private-input environment boundaries, not generated PAC layout assertions.
const HEAP: u32 = 0x2f06_0000;
const ROM_DATA: u32 = 0x2f07_ff90;
const CLOCK: u32 = 0x2010_f008;
const TIMER: u32 = 0x2010_f450;

fn input(name: &str, expected: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")));
    let hash = format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()));
    assert_eq!(hash, expected, "wrong {name} input");
    println!("input {name}: {hash}");
    path
}

fn symbol(image: &ExecutableImage, name: &str) -> u32 {
    image
        .symbol_address(name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn word(session: &ExecutionSession, image: &ExecutableImage, address: u32) -> u32 {
    u32::from_le_bytes(std::array::from_fn(|i| {
        session.byte(image, address + i as u32).unwrap()
    }))
}

fn map() -> MmioMap {
    MmioMap {
        registers: vec![],
        regions: vec![MmioRegion {
            name: "coex-clock-and-timers".into(),
            start: 0x2010_f000,
            end: 0x2010_f500,
            readable: true,
            writable: true,
        }],
    }
}

fn run(
    session: &mut ExecutionSession,
    image: &ExecutableImage,
    entry: &str,
    scenario: Scenario,
) -> ExecutionResult {
    let result = session
        .execute(image, &map(), entry, scenario)
        .unwrap_or_else(|error| panic!("INCOMPLETE {entry}: {error}"));
    println!(
        "{entry}: return={} steps={} events={:?}",
        result.return_value, result.steps, result.events
    );
    result
}

fn registers(values: &BTreeMap<u32, u32>, clock: u32) -> Scenario {
    Scenario {
        mmio_initial: [(CLOCK, clock)].into(),
        // Ordinary R/W storage models the CPU bus sequence only. No timer
        // progression, grant, RF preemption or command-bit semantics is assumed.
        device_models: values
            .iter()
            .map(|(&address, &initial_value)| {
                Arc::new(DeviceModelSpec::SelfClearing {
                    id: format!("bus-storage-{address:x}"),
                    address,
                    width: 32,
                    initial_value,
                    store_mask: u32::MAX,
                    command_mask: 0,
                }) as _
            })
            .collect(),
        max_steps: 10_000,
        ..Default::default()
    }
}

fn writes(result: &ExecutionResult) -> Vec<(u32, u32)> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Write { address, value, .. } => Some((*address, *value)),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "requires a linked coex-enabled vendor ELF and authenticated ROM; run through blobray-run"]
fn linked_initializer_and_phy_hooks_execute_through_rom_callbacks() {
    let elf = input(
        "OER_COEX_ELF",
        &std::env::var("OER_COEX_ELF_SHA256").expect("pin linked ELF SHA256"),
    );
    let rom = input(
        "OER_PHY_ROM",
        "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542",
    );
    let mut image = ExecutableImage::load_entry(&elf, "coex_rom_osi_funcs_init").unwrap();
    image.add_companion(&rom).unwrap();
    let mut session = ExecutionSession::default();
    let adapter = symbol(&image, "g_coex_adapter_funcs");
    let mut registration = Scenario {
        arguments: vec![adapter],
        persistent_memory: vec![
            MemoryRange {
                start: ROM_DATA,
                length: 0x20,
            },
            MemoryRange {
                start: HEAP,
                length: 0x38,
            },
        ],
        max_steps: 10_000,
        ..Default::default()
    };
    registration
        .memory_initial
        .extend((0..0x20).map(|offset| (ROM_DATA + offset, 0)));
    let registered = run(
        &mut session,
        &image,
        "esp_coex_adapter_register",
        registration,
    );
    assert_eq!(registered.return_value, 0);
    assert_eq!(
        word(&session, &image, symbol(&image, "g_coa_funcs_p")),
        adapter
    );
    // Model only OS allocation and logging. Execute the real linked callback
    // initializer, mapper, request, ROM release and hardware timer functions.
    let mut init = Scenario {
        max_steps: 10_000,
        ..Default::default()
    };
    init.call_responses.insert(
        "coexist_printf".into(),
        VecDeque::from([ModeledCallResponse::scalar(0)]),
    );
    init.call_responses.insert(
        "esp_coex_common_malloc_internal_wrapper".into(),
        VecDeque::from([ModeledCallResponse {
            allocation: Some(ModeledAllocation {
                address: HEAP,
                size_argument: 0,
                capacity: 0x38,
            }),
            ..Default::default()
        }]),
    );
    let installed = run(&mut session, &image, "coex_rom_osi_funcs_init", init);
    assert_eq!(installed.return_value, 0);
    assert_eq!(installed.allocations.len(), 1);
    assert_eq!(
        word(&session, &image, symbol(&image, "coexist_funcs")),
        HEAP
    );
    for (offset, name) in [
        (4, "coex_core_timer_idx_get"),
        (8, "coex_core_request"),
        (12, "coex_core_release"),
        (0x24, "coex_hw_timer_disable"),
    ] {
        assert_eq!(
            word(&session, &image, HEAP + offset),
            symbol(&image, name),
            "installed {name}"
        );
    }
    let repeat = Scenario {
        call_responses: [(
            "coexist_printf".into(),
            VecDeque::from([ModeledCallResponse::scalar(0)]),
        )]
        .into(),
        ..Default::default()
    };
    let repeated = run(&mut session, &image, "coex_rom_osi_funcs_init", repeat);
    assert_eq!(repeated.return_value, 0);
    assert!(
        repeated.allocations.is_empty(),
        "existing table must not be replaced"
    );
    let status_before = run(&mut session, &image, "coex_status_get", Scenario::default());
    assert!(
        status_before.events.is_empty(),
        "this status accessor reads software RAM, not RF grant MMIO"
    );

    for clock in [2, 0x102] {
        for initial in [0, 0xa5a5_5a5a] {
            let mut bus: BTreeMap<_, _> =
                [0, 4, 8, 12].map(|offset| (TIMER + offset, initial)).into();
            let acquired = run(
                &mut session,
                &image,
                "phy_acquire_grant_protect",
                registers(&bus, clock),
            );
            assert_eq!(acquired.return_value, 0);
            let requested = writes(&acquired);
            // Reviewed vendor bus behavior, not assertions about OER PAC names
            // or a statement that these writes have granted the physical RF.
            assert_eq!(
                requested,
                [
                    (TIMER, initial & !0x3000_0000),
                    (TIMER, (initial & !0x3f00_0000) | 0x0f00_0000),
                    (TIMER, (initial & 0xc000_0000) | 0x0f00_0000),
                    (TIMER + 4, initial & 0xff00_0000),
                    (TIMER + 12, initial & !1),
                    (TIMER + 8, initial | 1),
                ]
            );
            bus.extend(requested);
            let status_during = run(&mut session, &image, "coex_status_get", Scenario::default());
            assert!(status_during.events.is_empty());
            assert_eq!(status_during.return_value, status_before.return_value);
            let released = run(
                &mut session,
                &image,
                "phy_release_grant_protect",
                registers(&bus, clock),
            );
            assert_eq!(released.return_value, 0);
            assert_eq!(
                writes(&released),
                [(TIMER + 8, initial & !1), (TIMER + 12, initial | 1)]
            );
            assert!(
                released
                    .executed_pcs
                    .iter()
                    .any(|pc| (0x2f80_0000..0x2f84_0000).contains(pc)),
                "release must execute ROM instructions"
            );
        }
    }
}
