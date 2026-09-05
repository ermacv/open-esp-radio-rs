//! Attached-device inspection and exact flash transactions.

use crate::image::{Artifacts, program_from_env, run_command};
use crate::*;

use oer_firmware::flash::{
    OTA_0_OFFSET, OTA_SELECTOR_OFFSET, PARTITION_TABLE_OFFSET, ota0_selector_image,
};

pub(crate) fn status(root: &Path, lab: &crate::lab::config::LabConfig) -> Result<()> {
    device_status_at(&root.join("target/hil/esp32s31/device-status"), lab)
}

fn device_status_at(output: &Path, lab: &crate::lab::config::LabConfig) -> Result<()> {
    let capture = crate::session::SerialCapture::start_with_reset(&lab.device.serial);
    let result = (|| -> Result<_> {
        let capabilities = capture.prepare_protocol(lab)?;
        let operation = capture.query_operation_status(std::time::Duration::from_secs(10))?;
        let stack = capture.query_stack_usage(std::time::Duration::from_secs(10))?;
        Ok(serde_json::json!({
            "schema": crate::evidence::run::RUN_SCHEMA,
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
    firmware: &crate::evidence::verify::ArchivedFirmware,
    port: &Path,
) -> Result<()> {
    flash_replayed(
        root,
        &firmware.application_path,
        &firmware.run_id,
        firmware.image,
        port,
    )
}

pub(crate) fn flash_replayed(
    root: &Path,
    application: &Path,
    run_id: &str,
    image: crate::image::ImageClass,
    port: &Path,
) -> Result<()> {
    let output = root
        .join("target/hil/esp32s31/replay")
        .join(run_id)
        .join(image.id());
    flash_application(root, application, &output, port)
}

fn flash_application(root: &Path, application: &Path, output: &Path, port: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let partition_csv = root.join("platform/esp32s31/partitions/applications.csv");
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
    oer_firmware::flash::write_bin_command(&mut command, Some(port), address, image, after);
    run_command(&mut command, description)
}
