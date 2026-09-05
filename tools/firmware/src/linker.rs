pub fn configure_runtime(
    bin: &str,
    linker_dir: &std::path::Path,
    psram_data: bool,
    psram_code: bool,
    psram_task_stack: bool,
) {
    for file in [
        "rom/esp32s31-eco0.x",
        "runtime/link.x",
        "runtime/memory.x",
        "runtime/sections.x",
    ] {
        println!("cargo:rerun-if-changed={}", linker_dir.join(file).display());
    }
    println!("cargo:rustc-link-search={}", linker_dir.display());
    for argument in ["-Trom/esp32s31-eco0.x", "-Truntime/link.x", "--nmagic"] {
        println!("cargo:rustc-link-arg-bin={bin}={argument}");
    }
    assert!(
        psram_code || psram_data,
        "the Flash-code profile requires profile-psram-data"
    );
    assert!(
        !psram_task_stack || (psram_code && psram_data),
        "the PSRAM task-stack experiment requires PSRAM code and data"
    );

    let (code_origin, code_length): (u32, u32) = if psram_code {
        (0x5001_0000, 0x00ff_0000)
    } else {
        (0x4000_0140, 0x03ff_fec0)
    };
    let (data_origin, data_length): (u32, u32) = if psram_data {
        (0x5001_0000, 0x00ff_0000)
    } else {
        (0x2f00_0000, 0x0006_afc0)
    };
    for (symbol, value) in [
        ("RUNTIME_CODE_IN_PSRAM", u32::from(psram_code)),
        ("RUNTIME_CODE_ORIGIN", code_origin),
        ("RUNTIME_CODE_LENGTH", code_length),
        ("RUNTIME_DATA_IN_PSRAM", u32::from(psram_data)),
        ("RUNTIME_DATA_ORIGIN", data_origin),
        ("RUNTIME_DATA_LENGTH", data_length),
        ("PSRAM_TASK_STACKS", u32::from(psram_task_stack)),
    ] {
        println!("cargo:rustc-link-arg-bin={bin}=--defsym={symbol}={value}");
    }
}
