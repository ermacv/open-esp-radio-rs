use std::{
    collections::BTreeMap,
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

mod bidirectional;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const QUALIFIED_PROFILE: &str = "psram-code-psram-data";
const TARGET: &str = "riscv32imafc-unknown-none-elf";
const RUNTIME_BIN: &str = "open-esp-radio-hil-esp32s31-runtime";
const BOOTSTRAP_BIN: &str = "open-esp-radio-hil-esp32s31-bootstrap";
const RUNTIME_MAGIC: u32 = 0x3247_5453;
const RUNTIME_CRC_OFFSET: usize = 40;
const RUNTIME_HEADER_BYTES: usize = 44;
const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const OTA_SELECTOR_OFFSET: u32 = 0xd000;
const OTA_0_OFFSET: u32 = 0x1_0000;
const OTA_DATA_SIZE: usize = 0x2000;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let root = repository_root()?;
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("profiles") if arguments.next().is_none() => {
            println!("{QUALIFIED_PROFILE}");
            Ok(())
        }
        Some("doctor") if arguments.next().is_none() => doctor(&root),
        Some("build") => {
            let scenario = arguments
                .next()
                .map(|value| Scenario::parse(&value))
                .transpose()?
                .unwrap_or(Scenario::BootSmoke);
            if arguments.next().is_some() {
                return Err("usage: cargo hil build [boot-smoke|radio]".into());
            }
            let artifacts = build(&root, scenario)?;
            println!("profile={QUALIFIED_PROFILE}");
            println!("scenario={}", scenario.name());
            println!("runtime_elf={}", artifacts.runtime_elf.display());
            println!("runtime_bin={}", artifacts.runtime_bin.display());
            println!("bootstrap_elf={}", artifacts.bootstrap_elf.display());
            println!(
                "application_image={}",
                artifacts.application_image.display()
            );
            Ok(())
        }
        Some("flash") => {
            let (scenario, port) = parse_flash_arguments(arguments)?;
            let artifacts = build(&root, scenario)?;
            flash(&root, &artifacts, &port)?;
            println!("profile={QUALIFIED_PROFILE}");
            println!("scenario={}", scenario.name());
            println!("port={}", port.display());
            println!("flash=PASS");
            Ok(())
        }
        Some("traffic") => match arguments.next().as_deref() {
            Some("bidirectional") => bidirectional::run(arguments.collect(), &root),
            _ => Err("usage: cargo hil traffic bidirectional <ipv4> [options]".into()),
        },
        Some("help" | "--help" | "-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown HIL command `{command}`").into()),
    }
}

fn repository_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .ok_or_else(|| "HIL runner must live below the repository root".into())
}

fn doctor(root: &std::path::Path) -> Result<()> {
    let firmware = root.join("hil/esp32s31/Cargo.toml");
    if !firmware.is_file() {
        return Err(format!("missing embedded HIL workspace: {}", firmware.display()).into());
    }
    println!("repository={}", root.display());
    println!("firmware_workspace={}", firmware.display());
    println!("target={TARGET}");
    println!("qualified_profile={QUALIFIED_PROFILE}");
    for program in ["cargo", "llvm-objcopy", "llvm-nm", "espflash"] {
        require_program(program)?;
        println!("tool_{program}=PASS");
    }
    ensure_no_old_application_dependency(root)?;
    println!("old_application_dependency=ABSENT");
    println!("result=PASS");
    Ok(())
}

fn print_help() {
    println!(
        "Open ESP radio HIL\n\n\
         cargo hil profiles\n\
         cargo hil doctor\n\
         cargo hil build [boot-smoke|radio]\n\n\
         cargo hil flash [boot-smoke|radio|bidirectional] [--port /dev/ttyACM0]\n\
         cargo hil traffic bidirectional <ipv4> [options]\n\n\
         The build command compiles and packs both HIL stages, audits the \
         PSRAM/SRAM placement contract, and emits an ESP application image."
    );
}

struct Artifacts {
    output: PathBuf,
    runtime_elf: PathBuf,
    runtime_bin: PathBuf,
    bootstrap_elf: PathBuf,
    application_image: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    BootSmoke,
    Radio,
    Bidirectional,
}

impl Scenario {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "boot-smoke" => Ok(Self::BootSmoke),
            "radio" | "open-radio-hil" => Ok(Self::Radio),
            "bidirectional" | "open-radio-bidirectional" => Ok(Self::Bidirectional),
            _ => Err(format!("unsupported HIL scenario `{value}`").into()),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Radio => "open-radio-hil",
            Self::Bidirectional => "open-radio-bidirectional",
        }
    }

    const fn runtime_feature(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Radio | Self::Bidirectional => "open-radio-hil",
        }
    }
}

fn build(root: &Path, scenario: Scenario) -> Result<Artifacts> {
    ensure_no_old_application_dependency(root)?;
    let manifest = root.join("hil/esp32s31/Cargo.toml");
    let output =
        root.join("target/hil/esp32s31")
            .join(format!("{}-{}", QUALIFIED_PROFILE, scenario.name()));
    let runtime_target = output.join("cargo/runtime");
    let bootstrap_target = output.join("cargo/bootstrap");
    fs::create_dir_all(&output)?;

    let runtime_elf = runtime_target
        .join(TARGET)
        .join("release")
        .join(RUNTIME_BIN);
    let runtime_bin = output.join("runtime.bin");
    let bootstrap_elf = bootstrap_target
        .join(TARGET)
        .join("release")
        .join(BOOTSTRAP_BIN);
    let application_image = output.join("application.bin");

    let runtime_features = format!(
        "{},code-psram,profile-psram-data",
        scenario.runtime_feature()
    );
    let mut runtime = cargo_command();
    runtime
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", RUNTIME_BIN, "--release", "--target", TARGET])
        .args(["--no-default-features", "--features", &runtime_features])
        .env("CARGO_TARGET_DIR", &runtime_target);
    add_local_esp_hal_patches(&mut runtime, root);
    if scenario == Scenario::Bidirectional {
        runtime
            .env("OPEN_RADIO_RAW_MAC_BENCH", "1")
            // A benchmark image must not silently depend on the AP remaining
            // on the channel selected by an earlier HIL invocation.
            .env("OPEN_RADIO_FULL_SCAN", "1");
    }
    run_command(&mut runtime, "build stage-two runtime")?;
    require_file(&runtime_elf, "runtime ELF")?;

    let mut objcopy = Command::new(program_from_env("LLVM_OBJCOPY", "llvm-objcopy"));
    objcopy
        .args(["-O", "binary"])
        .arg(&runtime_elf)
        .arg(&runtime_bin);
    run_command(&mut objcopy, "flatten stage-two runtime")?;
    let crc = pack_runtime(&runtime_bin)?;
    let placement = audit_runtime(&runtime_elf, &runtime_bin)?;
    fs::write(output.join("placement.txt"), placement)?;

    let mut bootstrap = cargo_command();
    bootstrap
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", BOOTSTRAP_BIN, "--release", "--target", TARGET])
        .env("CARGO_TARGET_DIR", &bootstrap_target)
        .env("PSRAM_RUNTIME_BIN", absolute(&runtime_bin)?);
    add_local_esp_hal_patches(&mut bootstrap, root);
    run_command(&mut bootstrap, "build Flash/SRAM bootstrap")?;
    require_file(&bootstrap_elf, "bootstrap ELF")?;

    let partition_table = root.join("hil/esp32s31/partitions/hil.csv");
    let mut save_image = Command::new(program_from_env("ESPFLASH", "espflash"));
    save_image
        .args([
            "save-image",
            "--chip",
            "esp32s31",
            "--flash-mode",
            "qio",
            "--flash-freq",
            "80mhz",
            "--flash-size",
            "16mb",
            "--mmu-page-size",
            "65536",
            "--partition-table",
        ])
        .arg(&partition_table)
        .args(["--target-app-partition", "ota_0"])
        .arg(&bootstrap_elf)
        .arg(&application_image);
    run_command(&mut save_image, "encode ESP application image")?;
    audit_application_image(&application_image)?;

    eprintln!("runtime_crc32={crc:08x}");
    eprintln!("placement_audit=PASS");
    eprintln!("autonomous_source_graph=PASS");
    Ok(Artifacts {
        output,
        runtime_elf,
        runtime_bin,
        bootstrap_elf,
        application_image,
    })
}

fn parse_flash_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(Scenario, PathBuf)> {
    let mut scenario = Scenario::BootSmoke;
    let mut port = env::var_os("ESPFLASH_PORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/dev/ttyACM0"));
    let mut scenario_seen = false;
    while let Some(argument) = arguments.next() {
        if argument == "--port" {
            port = arguments
                .next()
                .map(PathBuf::from)
                .ok_or("--port requires a serial device")?;
        } else if !scenario_seen {
            scenario = Scenario::parse(&argument)?;
            scenario_seen = true;
        } else {
            return Err(format!("unknown flash argument `{argument}`").into());
        }
    }
    Ok((scenario, port))
}

fn flash(root: &Path, artifacts: &Artifacts, port: &Path) -> Result<()> {
    let partition_csv = root.join("hil/esp32s31/partitions/hil.csv");
    let partition_bin = artifacts.output.join("partitions.bin");
    let selector_bin = artifacts.output.join("otadata-ota0-valid.bin");

    let mut partition = Command::new(program_from_env("ESPFLASH", "espflash"));
    partition
        .args(["partition-table", "--to-binary", "--output"])
        .arg(&partition_bin)
        .arg(&partition_csv);
    run_command(&mut partition, "encode HIL partition table")?;
    fs::write(&selector_bin, ota0_selector_image())?;

    write_flash_binary(
        port,
        PARTITION_TABLE_OFFSET,
        &partition_bin,
        "no-reset",
        "write HIL partition table",
    )?;
    write_flash_binary(
        port,
        OTA_0_OFFSET,
        &artifacts.application_image,
        "no-reset",
        "write HIL application",
    )?;
    write_flash_binary(
        port,
        OTA_SELECTOR_OFFSET,
        &selector_bin,
        "hard-reset",
        "select HIL ota_0 image",
    )
}

fn write_flash_binary(
    port: &Path,
    address: u32,
    image: &Path,
    after: &str,
    description: &str,
) -> Result<()> {
    let mut command = Command::new(program_from_env("ESPFLASH", "espflash"));
    command
        .args([
            "write-bin",
            "--chip",
            "esp32s31",
            "--non-interactive",
            "--port",
        ])
        .arg(port)
        .args(["--after", after])
        .arg(format!("{address:#x}"))
        .arg(image);
    run_command(&mut command, description)
}

fn ota0_selector_image() -> [u8; OTA_DATA_SIZE] {
    let sequence = 1_u32;
    let mut image = [0xff; OTA_DATA_SIZE];
    image[0..4].copy_from_slice(&sequence.to_le_bytes());
    image[24..28].copy_from_slice(&2_u32.to_le_bytes());
    image[28..32].copy_from_slice(&crc32_idf(&sequence.to_le_bytes()).to_le_bytes());
    image
}

fn crc32_idf(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc ^ u32::MAX
}

fn cargo_command() -> Command {
    Command::new(program_from_env("CARGO", "cargo"))
}

fn program_from_env(variable: &str, fallback: &str) -> OsString {
    env::var_os(variable).unwrap_or_else(|| fallback.into())
}

fn add_local_esp_hal_patches(command: &mut Command, root: &Path) {
    let local = env::var_os("ESP_HAL_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.parent().unwrap_or(root).join("esp-hal"));
    let packages = [
        ("esp-bootloader-esp-idf", "esp-bootloader-esp-idf"),
        ("esp-hal", "esp-hal"),
        ("esp-sync", "esp-sync"),
    ];
    if !packages.iter().all(|(_, path)| local.join(path).is_dir()) {
        return;
    }
    for (package, path) in packages {
        command.arg("--config").arg(format!(
            "patch.\"https://github.com/ermacv/esp-hal\".{package}.path=\"{}\"",
            local.join(path).display()
        ));
    }
}

fn run_command(command: &mut Command, description: &str) -> Result<()> {
    eprintln!("==> {description}");
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{description} failed with {status}").into());
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    })
}

fn require_file(path: &Path, description: &str) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("missing {description}: {}", path.display()).into())
    }
}

fn require_program(program: &str) -> Result<()> {
    let status = Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("required program `{program}` is unavailable").into())
    }
}

fn ensure_no_old_application_dependency(root: &Path) -> Result<()> {
    for relative in [
        "hil/esp32s31/Cargo.toml",
        "hil/esp32s31/bootstrap/Cargo.toml",
        "hil/esp32s31/runtime/Cargo.toml",
        "hil/esp32s31/runtime-support/Cargo.toml",
        "hil/esp32s31/board/Cargo.toml",
    ] {
        let path = root.join(relative);
        let contents = fs::read_to_string(&path)?;
        if contents.contains("esp32s31_rust") {
            return Err(format!("{} still depends on esp32s31_rust", path.display()).into());
        }
    }
    Ok(())
}

fn pack_runtime(path: &Path) -> Result<u32> {
    let mut bytes = fs::read(path)?;
    if bytes.len() < RUNTIME_HEADER_BYTES {
        return Err("runtime image is shorter than its header".into());
    }
    if u32::from_le_bytes(bytes[0..4].try_into()?) != RUNTIME_MAGIC {
        return Err("runtime image has the wrong stage-two magic".into());
    }
    if u32::from_le_bytes(bytes[28..32].try_into()?) as usize != RUNTIME_HEADER_BYTES {
        return Err("runtime image has an incompatible header size".into());
    }
    bytes[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    let crc = crc32(&bytes);
    bytes[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    fs::write(path, bytes)?;
    let packed = fs::read(path)?;
    let stored = u32::from_le_bytes(packed[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].try_into()?);
    let mut verified = packed;
    verified[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    if stored != crc || crc32(&verified) != crc {
        return Err("runtime CRC did not survive packing".into());
    }
    Ok(crc)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn audit_runtime(elf: &Path, binary: &Path) -> Result<String> {
    let output = Command::new(program_from_env("LLVM_NM", "llvm-nm"))
        .args(["--defined-only", "--numeric-sort"])
        .arg(elf)
        .output()?;
    if !output.status.success() {
        return Err("llvm-nm failed while auditing runtime placement".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let mut symbols = BTreeMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(address) = words.next() else {
            continue;
        };
        let Some(_kind) = words.next() else { continue };
        let Some(name) = words.next() else { continue };
        if let Ok(address) = u64::from_str_radix(address, 16) {
            symbols.insert(name.to_owned(), address);
        }
    }
    let symbol = |name: &str| -> Result<u64> {
        symbols
            .get(name)
            .copied()
            .ok_or_else(|| format!("runtime ELF lacks `{name}`").into())
    };

    let image_start = symbol("__runtime_image_start")?;
    let payload_end = symbol("__runtime_payload_end")?;
    let text_start = symbol("__runtime_text_start")?;
    let text_end = symbol("__runtime_text_end")?;
    let entry = symbol("_runtime_start")?;
    let data_start = symbol("__runtime_data_start")?;
    let bss_end = symbol("__runtime_data_bss_end")?;
    let isr_start = symbol("__runtime_isr_start")?;
    let isr_end = symbol("__runtime_isr_end")?;
    let critical_start = symbol("__runtime_critical_data_start")?;
    let critical_bss_end = symbol("__runtime_critical_bss_end")?;
    let dma_start = symbol("__runtime_dma_data_start")?;
    let dma_end = symbol("__runtime_dma_bss_end")?;
    let stack_bottom = symbol("_stack_end")?;
    let stack_top = symbol("_stack_start")?;
    let binary_bytes = fs::metadata(binary)?.len();

    let in_sram = |start: u64, end: u64| start >= 0x2f00_0000 && end >= start && end <= 0x2f07_afc0;
    if image_start != 0x5001_0000
        || payload_end <= image_start
        || payload_end - image_start != binary_bytes
        || entry < text_start
        || entry >= text_end
        || data_start < 0x5000_0000
        || bss_end > 0x5100_0000
        || !in_sram(isr_start, isr_end)
        || !in_sram(critical_start, critical_bss_end)
        || !in_sram(dma_start, dma_end)
        || stack_top != 0x2f07_afc0
        || stack_top.saturating_sub(stack_bottom) < 0x1_0000
    {
        return Err("runtime ELF violates the PSRAM/PSRAM placement contract".into());
    }

    Ok(format!(
        "profile={QUALIFIED_PROFILE}\n\
         image={image_start:#010x}..{payload_end:#010x}\n\
         text={text_start:#010x}..{text_end:#010x}\n\
         data_start={data_start:#010x}\n\
         bss_end={bss_end:#010x}\n\
         isr={isr_start:#010x}..{isr_end:#010x}\n\
         critical={critical_start:#010x}..{critical_bss_end:#010x}\n\
         dma={dma_start:#010x}..{dma_end:#010x}\n\
         stack={stack_bottom:#010x}..{stack_top:#010x}\n\
         result=PASS\n"
    ))
}

fn audit_application_image(path: &Path) -> Result<()> {
    const APP_DESC_OFFSET: usize = 0x20;
    const APP_DESC_MMU_PAGE_LOG2_OFFSET: usize = 180;
    let bytes = fs::read(path)?;
    let end = APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET + 1;
    if bytes.len() < end
        || bytes[APP_DESC_OFFSET..APP_DESC_OFFSET + 4] != 0xabcd_5432_u32.to_le_bytes()
        || bytes[APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET] != 16
    {
        return Err("ESP application image has an invalid app descriptor or MMU page size".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_profile_name_is_stable() {
        assert_eq!(QUALIFIED_PROFILE, "psram-code-psram-data");
        assert_eq!(TARGET, "riscv32imafc-unknown-none-elf");
    }

    #[test]
    fn repository_layout_contains_the_embedded_workspace() {
        let root = repository_root().unwrap();
        assert!(root.join("hil/esp32s31/Cargo.toml").is_file());
    }

    #[test]
    fn scenario_names_are_stable() {
        assert_eq!(Scenario::parse("boot-smoke").unwrap(), Scenario::BootSmoke);
        assert_eq!(Scenario::parse("radio").unwrap(), Scenario::Radio);
        assert_eq!(Scenario::Radio.name(), "open-radio-hil");
    }

    #[test]
    fn runtime_crc_treats_the_checksum_field_as_zero() {
        let mut image = vec![0x5a; 128];
        image[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
        let expected = crc32(&image);
        image[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].copy_from_slice(&expected.to_le_bytes());
        image[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
        assert_eq!(crc32(&image), expected);
    }

    #[test]
    fn ota0_selector_uses_valid_idf_entry() {
        let image = ota0_selector_image();
        assert_eq!(u32::from_le_bytes(image[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(image[24..28].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(image[28..32].try_into().unwrap()),
            crc32_idf(&1_u32.to_le_bytes())
        );
        assert!(image[32..].iter().all(|byte| *byte == 0xff));
    }
}
