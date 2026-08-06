//! Derived SVD, PAC and PAC-binding publication commands.

use super::*;

pub(super) fn export_svd(
    arguments: Vec<String>,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let mut output = None;
    let mut profile = SvdExportProfile::Release;
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
            "--profile" => {
                profile = match take_value(&mut arguments, "--profile")?.as_str() {
                    "audit" => SvdExportProfile::Audit,
                    "release" => SvdExportProfile::Release,
                    value => {
                        return Err(format!(
                            "registers export-svd profile must be \"audit\" or \"release\", got {value:?}"
                        )
                        .into());
                    }
                };
            }
            "--reviewed-only" => profile = SvdExportProfile::Release,
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
    let (contents, summary) = workspace.render_svd(profile)?;
    super::super::super::generated_output::write_or_check(output, &contents, check, "SVD")?;
    println!(
        "SVD\tstatus={}\tprofile={}\tperipherals={}\tregisters={}\tfields={}\tpath={}",
        if check { "verified" } else { "written" },
        profile.label(),
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
    let (svd, svd_summary) = workspace.render_svd(SvdExportProfile::Release)?;
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
    let (svd, svd_summary) = workspace.render_svd(SvdExportProfile::Release)?;
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
