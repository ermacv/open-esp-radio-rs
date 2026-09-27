//! Run-local firmware preparation and exact-image flash ordering.

use std::path::Path;

use crate::Result;
use hil_core::{
    device, evidence::run::Failure, evidence::run::FailureKind, evidence::run::Outcome,
    evidence::run::PlannedFirmware, evidence::run::RunSession, evidence::verify::ArchivedFirmware,
    image::Artifacts, image::ImageClass, image::Integration, lab::config::LabConfig,
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

/// An image built before the stand's lease, awaiting its flash.
pub(crate) enum Built {
    Archived(Box<Artifacts>),
    Failed(Failure),
}

pub(crate) fn prepare_image(
    root: &Path,
    lab: &LabConfig,
    class: ImageClass,
    network: Integration,
    session: &mut RunSession,
) -> Result<Option<Failure>> {
    let built = build_image(class, network, session)?;
    flash_built(root, lab, class, built, session)
}

/// Build and archive one image class. This needs no hardware, so runs do it
/// before waiting for the stand.
pub(crate) fn build_image(
    class: ImageClass,
    network: Integration,
    session: &mut RunSession,
) -> Result<Built> {
    session.record_event("image-build-started", None, Some(class), None)?;
    let artifacts = match session.build_frozen_image(class, network) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            oer_process::check_cancelled()?;
            session.record_event(
                "image-build-failed",
                None,
                Some(class),
                Some(Outcome::Broken),
            )?;
            return Ok(Built::Failed(Failure::new(
                FailureKind::ImageBuild,
                error.to_string(),
            )));
        }
    };
    Ok(Built::Archived(Box::new(archive_built(
        class, artifacts, session,
    )?)))
}

pub(crate) fn flash_built(
    root: &Path,
    lab: &LabConfig,
    class: ImageClass,
    built: Built,
    session: &mut RunSession,
) -> Result<Option<Failure>> {
    let artifacts = match built {
        Built::Archived(artifacts) => artifacts,
        Built::Failed(failure) => return Ok(Some(failure)),
    };
    let failure = flash_archived_build(class, &artifacts, session, |artifacts| {
        device::flash(root, artifacts, &lab.device.serial)
    })?;
    if failure.is_none() {
        let repository = session.repository();
        hil_core::lab::lock::record_flash(
            &lab.device.serial,
            class.id(),
            &artifacts.application_image,
            Some(repository.commit.clone()),
            Some(repository.dirty),
            format!("run {}", session.id()),
        );
    }
    Ok(failure)
}

#[cfg(test)]
fn archive_and_flash_built(
    class: ImageClass,
    artifacts: Artifacts,
    session: &mut RunSession,
    flash: impl FnOnce(&Artifacts) -> Result<()>,
) -> Result<Option<Failure>> {
    let artifacts = archive_built(class, artifacts, session)?;
    flash_archived_build(class, &artifacts, session, flash)
}

fn archive_built(
    class: ImageClass,
    mut artifacts: Artifacts,
    session: &mut RunSession,
) -> Result<Artifacts> {
    artifacts.application_image = session.record_firmware(class, &artifacts)?;
    session.record_event(
        "image-build-finished",
        None,
        Some(class),
        Some(Outcome::Passed),
    )?;
    Ok(artifacts)
}

fn flash_archived_build(
    class: ImageClass,
    artifacts: &Artifacts,
    session: &mut RunSession,
    flash: impl FnOnce(&Artifacts) -> Result<()>,
) -> Result<Option<Failure>> {
    session.record_event("image-flash-started", None, Some(class), None)?;
    if let Err(error) = flash(artifacts) {
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
    let mut flashed = None;
    let failure = import_and_flash_replay(archived, session, |application| {
        device::flash_replayed(
            root,
            application,
            &run_id,
            archived.image,
            &lab.device.serial,
        )?;
        flashed = Some(application.to_owned());
        Ok(())
    })?;
    if let Some(application) = flashed {
        hil_core::lab::lock::record_flash(
            &lab.device.serial,
            archived.image.id(),
            &application,
            None,
            None,
            format!("run {run_id} replaying run {}", archived.run_id),
        );
    }
    Ok(failure)
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
