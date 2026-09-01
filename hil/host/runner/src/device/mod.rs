//! Attached-device inspection and exact flash transactions.

use crate::image::{Artifacts, program_from_env, run_command};
use crate::*;

const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const OTA_SELECTOR_OFFSET: u32 = 0xd000;
const OTA_0_OFFSET: u32 = 0x1_0000;
const OTA_DATA_SIZE: usize = 0x2000;

pub(crate) fn status(root: &Path, lab: &transport::lab_config::LabConfig) -> Result<()> {
    device_status_at(&root.join("target/hil/esp32s31/device-status"), lab)
}

fn device_status_at(output: &Path, lab: &transport::lab_config::LabConfig) -> Result<()> {
    let capture = evidence::traffic_capture::SerialCapture::start_with_reset(&lab.device.serial);
    let result = (|| -> Result<_> {
        let capabilities = capture.prepare_protocol(lab)?;
        let operation = capture.query_operation_status(std::time::Duration::from_secs(10))?;
        let stack = capture.query_stack_usage(std::time::Duration::from_secs(10))?;
        Ok(serde_json::json!({
            "schema": reporting::run::RUN_SCHEMA,
            "protocol_version": open_esp_radio_hil_protocol::PROTOCOL_VERSION,
            "capabilities": capabilities,
            "operation": operation,
            "stack": stack,
            "uart_log": output.join("uart.log"),
        }))
    })();
    let capture_result = capture.finish_to(output);
    let report = result?;
    capture_result?;
    crate::emit_json(&report, true)
}

pub(crate) fn flash(root: &Path, artifacts: &Artifacts, port: &Path) -> Result<()> {
    flash_application(root, &artifacts.application_image, &artifacts.output, port)
}

pub(crate) fn flash_archived(
    root: &Path,
    firmware: &reporting::verification::ArchivedFirmware,
    port: &Path,
) -> Result<()> {
    let output = root
        .join("target/hil/esp32s31/replay")
        .join(&firmware.run_id)
        .join(firmware.image.id());
    flash_application(root, &firmware.application_path, &output, port)
}

fn flash_application(root: &Path, application: &Path, output: &Path, port: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let partition_csv = root.join("hil/targets/esp32s31/partitions/hil.csv");
    let partition_bin = output.join("partitions.bin");
    let selector_bin = output.join("otadata-ota0-valid.bin");

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
        application,
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

#[cfg(test)]
mod tests {
    use super::*;

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
