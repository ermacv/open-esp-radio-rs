use std::{
    env,
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use svd2rust::{
    config::{Config, RustEdition},
    Target,
};
use svd_parser::svd::{MaybeArray, RegisterCluster, RegisterProperties};

const USAGE: &str = "usage: cargo pac-gen [--check]";
const MMIO_WINDOWS: [(&str, u64, u64); 1] = [("modem-radio-core", 0x2010_0000, 0x2020_0000)];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("pac-gen must remain under tools/pac-gen")
        .to_owned()
}

fn format_generated(source: &str) -> Result<String, Box<dyn Error>> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped rustfmt stdin must exist")
        .write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(format!("rustfmt exited with {}", output.status).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn mmio_window(start: u64, end_exclusive: u64) -> Option<&'static str> {
    MMIO_WINDOWS
        .iter()
        .find(|(_, window_start, window_end)| {
            start >= *window_start && end_exclusive <= *window_end
        })
        .map(|(name, _, _)| *name)
}

fn register_size_bytes(
    properties: RegisterProperties,
    inherited_size_bits: Option<u32>,
) -> Result<u64, Box<dyn Error>> {
    let size_bits = properties
        .size
        .or(inherited_size_bits)
        .ok_or("SVD register has no size and no inherited size")?;
    if size_bits == 0 {
        return Err("SVD register size must not be zero".into());
    }
    Ok(u64::from(size_bits).div_ceil(8))
}

fn validate_register(
    peripheral_name: &str,
    register_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    register_offset: u32,
    properties: RegisterProperties,
    inherited_size_bits: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    let start = peripheral_base
        .checked_add(parent_offset)
        .and_then(|address| address.checked_add(u64::from(register_offset)))
        .ok_or("SVD register address overflow")?;
    let end_exclusive = start
        .checked_add(register_size_bytes(properties, inherited_size_bits)?)
        .ok_or("SVD register end address overflow")?;
    if mmio_window(start, end_exclusive).is_none() {
        return Err(format!(
            "SVD register {peripheral_name}.{register_name} spans \
             0x{start:08x}..0x{end_exclusive:08x}, outside the evidenced \
             ESP32-S31 MMIO windows"
        )
        .into());
    }
    Ok(())
}

fn validate_children(
    peripheral_name: &str,
    peripheral_base: u64,
    parent_offset: u64,
    children: &[RegisterCluster],
    inherited_size_bits: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => match register {
                MaybeArray::Single(info) => validate_register(
                    peripheral_name,
                    &info.name,
                    peripheral_base,
                    parent_offset,
                    info.address_offset,
                    info.properties,
                    inherited_size_bits,
                )?,
                MaybeArray::Array(info, dim) => {
                    for index in 0..dim.dim {
                        let offset = info
                            .address_offset
                            .checked_add(index.saturating_mul(dim.dim_increment))
                            .ok_or("SVD register-array offset overflow")?;
                        validate_register(
                            peripheral_name,
                            &info.name,
                            peripheral_base,
                            parent_offset,
                            offset,
                            info.properties,
                            inherited_size_bits,
                        )?;
                    }
                }
            },
            RegisterCluster::Cluster(cluster) => {
                let cluster_size = cluster
                    .default_register_properties
                    .size
                    .or(inherited_size_bits);
                match cluster {
                    MaybeArray::Single(info) => validate_children(
                        peripheral_name,
                        peripheral_base,
                        parent_offset + u64::from(info.address_offset),
                        &info.children,
                        cluster_size,
                    )?,
                    MaybeArray::Array(info, dim) => {
                        for index in 0..dim.dim {
                            let offset = info
                                .address_offset
                                .checked_add(index.saturating_mul(dim.dim_increment))
                                .ok_or("SVD cluster-array offset overflow")?;
                            validate_children(
                                peripheral_name,
                                peripheral_base,
                                parent_offset + u64::from(offset),
                                &info.children,
                                cluster_size,
                            )?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_mmio_windows(input: &str) -> Result<(), Box<dyn Error>> {
    let device = svd_parser::parse(input)?;
    for peripheral in &device.peripherals {
        let peripheral_size = peripheral
            .default_register_properties
            .size
            .or(device.default_register_properties.size);
        let validate_instance = |base_address: u64| -> Result<(), Box<dyn Error>> {
            if mmio_window(base_address, base_address + 1).is_none() {
                return Err(format!(
                    "SVD peripheral {} starts at 0x{base_address:08x}, outside \
                     the evidenced ESP32-S31 MMIO windows",
                    peripheral.name
                )
                .into());
            }
            if let Some(registers) = &peripheral.registers {
                validate_children(
                    &peripheral.name,
                    base_address,
                    0,
                    registers,
                    peripheral_size,
                )?;
            }
            Ok(())
        };

        match peripheral {
            MaybeArray::Single(info) => validate_instance(info.base_address)?,
            MaybeArray::Array(info, dim) => {
                for index in 0..dim.dim {
                    validate_instance(
                        info.base_address + u64::from(index.saturating_mul(dim.dim_increment)),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let check = match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => return Err(USAGE.into()),
    };

    let root = repository_root();
    let svd_path = root.join("svd/esp32s31-radio.svd");
    let output_path = root.join("crates/open-esp-radio-svd-esp32s31/src/lib.rs");
    let input = fs::read_to_string(&svd_path)?;
    validate_mmio_windows(&input)?;

    let mut config = Config::default();
    config.edition = RustEdition::E2021;
    config.target = Target::None;
    config.strict = true;
    let generated = format_generated(&svd2rust::generate(&input, &config)?.lib_rs)?;

    if check {
        let checked_in = fs::read_to_string(&output_path)?;
        if checked_in != generated {
            return Err(format!(
                "{} differs from {}; run `cargo pac-gen`",
                output_path.display(),
                svd_path.display()
            )
            .into());
        }
    } else {
        fs::create_dir_all(
            output_path
                .parent()
                .expect("generated source path must have a parent"),
        )?;
        fs::write(&output_path, generated)?;
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("PAC generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mmio_window;

    #[test]
    fn accepts_the_remaining_custom_pac_decode_window() {
        assert_eq!(
            mmio_window(0x2010_0000, 0x2010_0004),
            Some("modem-radio-core")
        );
    }

    #[test]
    fn rejects_holes_and_cross_window_registers() {
        assert_eq!(mmio_window(0x2000_0000, 0x2000_0004), None);
        assert_eq!(mmio_window(0x2020_0000, 0x2020_0004), None);
        assert_eq!(mmio_window(0x201f_fffc, 0x2020_0004), None);
        assert_eq!(mmio_window(0x2058_7000, 0x2058_7004), None);
        assert_eq!(mmio_window(0x2070_4000, 0x2070_4004), None);
        assert_eq!(mmio_window(0x2081_8000, 0x2081_8004), None);
        assert_eq!(mmio_window(0x2090_0000, 0x2090_0004), None);
    }
}
