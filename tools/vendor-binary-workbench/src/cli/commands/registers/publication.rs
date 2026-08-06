//! Derived SVD, PAC and PAC-binding publication commands.

use super::*;

pub(crate) struct PreparedPublication {
    output: PathBuf,
    contents: String,
    kind: &'static str,
    report: PublicationReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationReadiness {
    Current,
    Missing,
    Stale,
}

impl PublicationReadiness {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Current => "ready",
            Self::Missing => "missing",
            Self::Stale => "stale",
        }
    }
}

enum PublicationReport {
    Svd {
        summary: SvdExportSummary,
    },
    Pac {
        target: PacTarget,
        edition: PacEdition,
        summary: SvdExportSummary,
        api_pack: Option<PathBuf>,
    },
    Bindings {
        crate_name: String,
        summary: SvdExportSummary,
    },
}

impl PreparedPublication {
    pub(crate) fn readiness(&self) -> Result<PublicationReadiness> {
        match std::fs::read_to_string(&self.output) {
            Ok(existing) if existing == self.contents => Ok(PublicationReadiness::Current),
            Ok(_) => Ok(PublicationReadiness::Stale),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(PublicationReadiness::Missing)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn output(&self) -> &std::path::Path {
        &self.output
    }

    pub(crate) fn write_or_check(&self, check: bool) -> Result<bool> {
        super::super::super::generated_output::write_or_check(
            &self.output,
            &self.contents,
            check,
            self.kind,
        )?;
        let status = if check { "verified" } else { "written" };
        match &self.report {
            PublicationReport::Svd { summary } => println!(
                "SVD\tstatus={status}\tperipherals={}\tregisters={}\tfields={}\tpath={}",
                summary.peripherals,
                summary.registers,
                summary.fields,
                self.output.display()
            ),
            PublicationReport::Pac {
                target,
                edition,
                summary,
                api_pack,
            } => println!(
                "PAC\tstatus={status}\ttarget={}\tedition={}\tperipherals={}\tregisters={}\tapi-pack={}\tpath={}",
                target.label(),
                edition.label(),
                summary.peripherals,
                summary.registers,
                api_pack
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
                self.output.display()
            ),
            PublicationReport::Bindings {
                crate_name,
                summary,
            } => println!(
                "PAC-BINDINGS\tstatus={status}\tcrate={crate_name}\tperipherals={}\tregisters={}\tpath={}",
                summary.peripherals,
                summary.registers,
                self.output.display()
            ),
        }
        Ok(true)
    }
}

pub(crate) fn prepare_project_svd(
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<PreparedPublication> {
    let output = paths
        .svd_output
        .clone()
        .ok_or("project SVD publication is not configured")?;
    let workspace = load_release_workspace(paths, "release SVD")?;
    let (contents, summary) = workspace.render_svd()?;
    Ok(PreparedPublication {
        output,
        contents,
        kind: "SVD",
        report: PublicationReport::Svd { summary },
    })
}

pub(crate) fn prepare_project_pac(
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<PreparedPublication> {
    let configured = paths
        .pac
        .as_ref()
        .ok_or("project PAC publication is not configured")?;
    let target = PacTarget::parse(&configured.target)?;
    let edition = PacEdition::parse(&configured.edition)?;
    let api_pack = paths
        .api_pack
        .as_deref()
        .map(PacApiPack::load)
        .transpose()?;
    let workspace = load_release_workspace(paths, "PAC generation")?;
    let (svd, summary) = workspace.render_svd()?;
    let contents = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC",
        report: PublicationReport::Pac {
            target,
            edition,
            summary,
            api_pack: paths.api_pack.clone(),
        },
    })
}

pub(crate) fn prepare_project_bindings(
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<PreparedPublication> {
    let configured = paths
        .bindings
        .as_ref()
        .ok_or("project PAC binding publication is not configured")?;
    let workspace = load_release_workspace(paths, "PAC binding generation")?;
    let (svd, summary) = workspace.render_svd()?;
    let contents =
        open_esp_radio_register_model::generate_pac_binding_index(&svd, &configured.crate_name)?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC binding index",
        report: PublicationReport::Bindings {
            crate_name: configured.crate_name.clone(),
            summary,
        },
    })
}

fn load_release_workspace(
    paths: &crate::project::RegisterWorkspacePaths,
    operation: &str,
) -> Result<ProjectRegisterWorkspace> {
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let summary = workspace.summary()?;
    if summary.unreviewed != 0 {
        return Err(format!(
            "{operation} denied {} unreviewed MMIO observations",
            summary.unreviewed
        )
        .into());
    }
    Ok(workspace)
}

pub(super) fn export_svd(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut check = false;
    let mut deny_unreviewed = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--deny-unreviewed" => deny_unreviewed = true,
            "--check" => check = true,
            _ => return Err(format!("unknown registers export-svd option: {argument}").into()),
        }
    }
    let output = output
        .as_deref()
        .or(paths.svd_output.as_deref())
        .ok_or("registers export-svd requires --output PATH or [registers.svd] output")?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = print_summary(&workspace, paths)?;
    if deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(format!(
            "release SVD denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )
        .into());
    }
    let (contents, summary) = workspace.render_svd()?;
    super::super::super::generated_output::write_or_check(output, &contents, check, "SVD")?;
    println!(
        "SVD\tstatus={}\tperipherals={}\tregisters={}\tfields={}\tpath={}",
        if check { "verified" } else { "written" },
        summary.peripherals,
        summary.registers,
        summary.fields,
        output.display()
    );
    Ok(true)
}

pub(super) fn generate_pac_source(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut target = None;
    let mut edition = None;
    let mut api_pack = None;
    let mut no_api_pack = false;
    let mut check = false;
    let mut deny_unreviewed = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--target" => {
                target = Some(PacTarget::parse(&take_value(&mut arguments, "--target")?)?);
            }
            "--edition" => {
                edition = Some(PacEdition::parse(&take_value(
                    &mut arguments,
                    "--edition",
                )?)?);
            }
            "--api-pack" => {
                if api_pack.is_some() || no_api_pack {
                    return Err("duplicate or conflicting --api-pack/--no-api-pack".into());
                }
                api_pack = Some(PathBuf::from(take_value(&mut arguments, "--api-pack")?));
            }
            "--no-api-pack" => {
                if no_api_pack || api_pack.is_some() {
                    return Err("duplicate or conflicting --api-pack/--no-api-pack".into());
                }
                no_api_pack = true;
            }
            "--check" => check = true,
            "--deny-unreviewed" => deny_unreviewed = true,
            _ => return Err(format!("unknown registers generate-pac option: {argument}").into()),
        }
    }
    let configured = paths.pac.as_ref();
    let output = output
        .as_deref()
        .or_else(|| configured.map(|pac| pac.output.as_path()))
        .ok_or("registers generate-pac requires --output PATH or [registers.pac] output")?;
    let target = target
        .map(Ok)
        .unwrap_or_else(|| PacTarget::parse(configured.map_or("none", |pac| &pac.target)))?;
    let edition = edition
        .map(Ok)
        .unwrap_or_else(|| PacEdition::parse(configured.map_or("2024", |pac| &pac.edition)))?;
    let api_pack_path = if no_api_pack {
        None
    } else {
        api_pack.as_deref().or(paths.api_pack.as_deref())
    };
    let api_pack = api_pack_path.map(PacApiPack::load).transpose()?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = print_summary(&workspace, paths)?;
    if deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(format!(
            "PAC generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )
        .into());
    }
    let (svd, svd_summary) = workspace.render_svd()?;
    let source = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    super::super::super::generated_output::write_or_check(output, &source, check, "PAC")?;
    println!(
        "PAC\tstatus={}\ttarget={}\tedition={}\tperipherals={}\tregisters={}\tapi-pack={}\tpath={}",
        if check { "verified" } else { "written" },
        target.label(),
        edition.label(),
        svd_summary.peripherals,
        svd_summary.registers,
        api_pack_path.map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
        output.display()
    );
    Ok(true)
}

pub(super) fn generate_bindings(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut crate_name = None;
    let mut check = false;
    let mut deny_unreviewed = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                if output.is_some() {
                    return Err("duplicate --output".into());
                }
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            "--crate-name" => {
                if crate_name.is_some() {
                    return Err("duplicate --crate-name".into());
                }
                crate_name = Some(take_value(&mut arguments, "--crate-name")?);
            }
            "--check" => check = true,
            "--deny-unreviewed" => deny_unreviewed = true,
            _ => {
                return Err(
                    format!("unknown registers generate-bindings option: {argument}").into(),
                );
            }
        }
    }
    let configured = paths.bindings.as_ref();
    let output = output
        .as_deref()
        .or_else(|| configured.map(|bindings| bindings.output.as_path()))
        .ok_or(
            "registers generate-bindings requires --output PATH or [registers.bindings] output",
        )?;
    let crate_name = crate_name
        .as_deref()
        .or_else(|| configured.map(|bindings| bindings.crate_name.as_str()))
        .ok_or(
            "registers generate-bindings requires --crate-name NAME or [registers.bindings] crate-name",
        )?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = print_summary(&workspace, paths)?;
    if deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(format!(
            "PAC binding generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )
        .into());
    }
    let (svd, svd_summary) = workspace.render_svd()?;
    let contents = open_esp_radio_register_model::generate_pac_binding_index(&svd, crate_name)?;
    super::super::super::generated_output::write_or_check(
        output,
        &contents,
        check,
        "PAC binding index",
    )?;
    println!(
        "PAC-BINDINGS\tstatus={}\tcrate={}\tperipherals={}\tregisters={}\tpath={}",
        if check { "verified" } else { "written" },
        crate_name,
        svd_summary.peripherals,
        svd_summary.registers,
        output.display()
    );
    Ok(true)
}
