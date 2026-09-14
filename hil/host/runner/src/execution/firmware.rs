//! Run-local firmware preparation and exact-image flash ordering.

use std::path::Path;

use crate::{
    Result, device,
    evidence::{
        run::{Failure, FailureKind, Outcome, PlannedFirmware, RunSession},
        verify::ArchivedFirmware,
    },
    image::{self, Artifacts, ImageClass, Integration},
    lab::config::LabConfig,
};

pub(crate) enum RunFirmware {
    BuildCurrent(Integration),
    Replay(Box<ArchivedFirmware>),
}

impl RunFirmware {
    pub(crate) fn plan(&self) -> PlannedFirmware {
        match self {
            Self::BuildCurrent(_) => PlannedFirmware::BuildCurrent,
            Self::Replay(firmware) => PlannedFirmware::Replay {
                source_run_id: firmware.run_id.clone(),
                image: firmware.image,
                build_id: firmware.build_id.clone(),
                application_sha256: firmware.application_sha256.clone(),
            },
        }
    }
}

pub(crate) fn prepare_run_image(
    root: &Path,
    lab: &LabConfig,
    class: ImageClass,
    firmware: &RunFirmware,
    session: &mut RunSession,
) -> Result<Option<Failure>> {
    match firmware {
        RunFirmware::BuildCurrent(network) => prepare_image(root, lab, class, *network, session),
        RunFirmware::Replay(archived) => prepare_replayed_image(root, lab, archived, session),
    }
}

pub(crate) fn prepare_image(
    root: &Path,
    lab: &LabConfig,
    class: ImageClass,
    network: Integration,
    session: &mut RunSession,
) -> Result<Option<Failure>> {
    session.record_event("image-build-started", None, Some(class), None)?;
    let artifacts = match image::build(root, class, network) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            oer_process::check_cancelled()?;
            session.record_event(
                "image-build-failed",
                None,
                Some(class),
                Some(Outcome::Broken),
            )?;
            return Ok(Some(Failure::new(
                FailureKind::ImageBuild,
                error.to_string(),
            )));
        }
    };
    archive_and_flash_built(class, artifacts, session, |artifacts| {
        device::flash(root, artifacts, &lab.device.serial)
    })
}

fn archive_and_flash_built(
    class: ImageClass,
    mut artifacts: Artifacts,
    session: &mut RunSession,
    flash: impl FnOnce(&Artifacts) -> Result<()>,
) -> Result<Option<Failure>> {
    artifacts.application_image = session.record_firmware(
        class,
        &artifacts.application_image,
        &artifacts.runtime_elf,
        &artifacts.runtime_bin,
        &artifacts.bootstrap_elf,
        (
            &artifacts.effective_embedded_lock,
            &artifacts.effective_bootstrap_lock,
        ),
    )?;
    session.record_event(
        "image-build-finished",
        None,
        Some(class),
        Some(Outcome::Passed),
    )?;
    session.record_event("image-flash-started", None, Some(class), None)?;
    if let Err(error) = flash(&artifacts) {
        oer_process::check_cancelled()?;
        session.record_event(
            "image-flash-failed",
            None,
            Some(class),
            Some(Outcome::Broken),
        )?;
        return Ok(Some(Failure::new(
            FailureKind::ImageFlash,
            error.to_string(),
        )));
    }
    session.record_event(
        "image-flash-finished",
        None,
        Some(class),
        Some(Outcome::Passed),
    )?;
    Ok(None)
}

fn prepare_replayed_image(
    root: &Path,
    lab: &LabConfig,
    archived: &ArchivedFirmware,
    session: &mut RunSession,
) -> Result<Option<Failure>> {
    let run_id = session.id().to_owned();
    import_and_flash_replay(archived, session, |application| {
        device::flash_replayed(
            root,
            application,
            &run_id,
            archived.image,
            &lab.device.serial,
        )
    })
}

fn import_and_flash_replay(
    archived: &ArchivedFirmware,
    session: &mut RunSession,
    flash: impl FnOnce(&Path) -> Result<()>,
) -> Result<Option<Failure>> {
    session.record_event(
        "image-replay-import-started",
        None,
        Some(archived.image),
        None,
    )?;
    let application = match session.record_replayed_firmware(archived) {
        Ok(application) => application,
        Err(error) => {
            oer_process::check_cancelled()?;
            session.record_event(
                "image-replay-import-failed",
                None,
                Some(archived.image),
                Some(Outcome::Broken),
            )?;
            return Err(error);
        }
    };
    session.record_event(
        "image-replay-import-finished",
        None,
        Some(archived.image),
        Some(Outcome::Passed),
    )?;
    session.record_event("image-flash-started", None, Some(archived.image), None)?;
    if let Err(error) = flash(&application) {
        oer_process::check_cancelled()?;
        session.record_event(
            "image-flash-failed",
            None,
            Some(archived.image),
            Some(Outcome::Broken),
        )?;
        return Ok(Some(Failure::new(
            FailureKind::ImageFlash,
            error.to_string(),
        )));
    }
    session.record_event(
        "image-flash-finished",
        None,
        Some(archived.image),
        Some(Outcome::Passed),
    )?;
    Ok(None)
}

#[cfg(test)]
mod tests;
