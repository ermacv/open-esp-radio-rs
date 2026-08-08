use std::fs;

use super::options::DEFAULT_RUST_TARGET;
use super::*;

#[test]
fn parses_generic_sources_ranges_and_defaults() {
    let options = resolve_options(ProjectInitArgs {
        directory: "project".into(),
        id: "radio-rev0".to_owned(),
        mmio: vec!["radio=0x20000000..0x20010000".parse().unwrap()],
        source: vec!["rom".parse().unwrap(), "archive".parse().unwrap()],
        rust_target: None,
        pac_crate_name: None,
        import_svd: None,
    })
    .unwrap();
    assert_eq!(options.sources, ["rom", "archive"]);
    assert_eq!(options.pac_crate_name, "radio_rev0_pac");
    assert_eq!(options.rust_target, DEFAULT_RUST_TARGET);
    assert_eq!(options.ranges[0].start, 0x2000_0000);
}

#[test]
fn creates_a_valid_project_and_refuses_to_overwrite_it() {
    let parent = std::env::temp_dir().join(format!(
        "vendor-workbench-project-init-{}",
        std::process::id()
    ));
    if parent.exists() {
        fs::remove_dir_all(&parent).unwrap();
    }
    fs::create_dir_all(&parent).unwrap();
    let directory = parent.join("radio");
    let arguments = ProjectInitArgs {
        directory: directory.clone(),
        id: "radio".to_owned(),
        mmio: vec!["radio=0x20000000..0x20010000".parse().unwrap()],
        source: Vec::new(),
        rust_target: None,
        pac_crate_name: None,
        import_svd: None,
    };

    assert!(run(arguments.clone()).unwrap());
    let project = ProjectSpec::load(&directory.join(DEFAULT_PROJECT_MANIFEST)).unwrap();
    let target = TargetSpec::load(&directory.join("target.spec")).unwrap();
    let memory = MemoryMap::load(&directory.join("memory.toml")).unwrap();
    let model = RegisterModel::load(&project.registers.as_ref().unwrap().model).unwrap();
    assert_eq!(project.ir_profiles.len(), 1);
    assert_eq!(
        project.platform_pack.as_ref().map(|pack| pack.id.as_str()),
        Some("radio-platform")
    );
    assert!(project.interfaces.is_some());
    assert!(project.functions.is_some());
    target.require_available_backend().unwrap();
    assert_eq!(memory.mmio_ranges().unwrap().len(), 1);
    assert_eq!(model.render_svd().unwrap().1.peripherals, 1);
    let containment = crate::registers::validate_register_memory_map(
        project.registers.as_ref().unwrap(),
        Some(&memory),
    )
    .unwrap()
    .unwrap();
    assert_eq!(containment.registers, 0);
    assert_eq!(containment.mmio_regions, 1);
    assert!(
        run(arguments)
            .unwrap_err()
            .to_string()
            .contains("overwrite")
    );

    fs::remove_dir_all(parent).unwrap();
}

#[test]
fn rejects_overlapping_ranges_before_creating_a_directory() {
    let directory = std::env::temp_dir().join(format!(
        "vendor-workbench-project-init-overlap-{}",
        std::process::id()
    ));
    let error = resolve_options(ProjectInitArgs {
        directory: directory.clone(),
        id: "radio".to_owned(),
        mmio: vec![
            "one=0x20000000..0x20001000".parse().unwrap(),
            "two=0x20000800..0x20002000".parse().unwrap(),
        ],
        source: Vec::new(),
        rust_target: None,
        pac_crate_name: None,
        import_svd: None,
    })
    .unwrap_err();
    assert!(error.to_string().contains("overlap"));
    assert!(!directory.exists());
}

#[test]
fn imported_svd_must_fit_the_declared_mmio_map() {
    let parent = std::env::temp_dir().join(format!(
        "vendor-workbench-project-init-import-{}",
        std::process::id()
    ));
    if parent.exists() {
        fs::remove_dir_all(&parent).unwrap();
    }
    fs::create_dir_all(&parent).unwrap();
    let input = parent.join("outside.svd");
    fs::write(
        &input,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<device schemaVersion="1.3" xmlns:xs="http://www.w3.org/2001/XMLSchema-instance" xs:noNamespaceSchemaLocation="CMSIS-SVD.xsd">
  <name>TEST</name>
  <version>0.1</version>
  <description>Import containment fixture</description>
  <addressUnitBits>8</addressUnitBits>
  <width>32</width>
  <peripherals>
    <peripheral>
      <name>OUTSIDE</name>
      <baseAddress>0x30000000</baseAddress>
      <registers>
        <register>
          <name>CONTROL</name>
          <addressOffset>0</addressOffset>
          <size>0x20</size>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#,
    )
    .unwrap();
    let directory = parent.join("project");
    let error = run(ProjectInitArgs {
        directory: directory.clone(),
        id: "radio".to_owned(),
        mmio: vec!["radio=0x20000000..0x20010000".parse().unwrap()],
        source: Vec::new(),
        rust_target: None,
        pac_crate_name: None,
        import_svd: Some(input),
    })
    .unwrap_err();
    assert!(error.to_string().contains("outside project MMIO"));
    assert!(!directory.exists());

    fs::remove_dir_all(parent).unwrap();
}
