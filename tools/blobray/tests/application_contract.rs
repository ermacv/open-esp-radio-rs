//! Public facade selection contracts, independent of CLI argument resolution.

use std::{io::Write as _, path::Path};

use blobray::{AnalyzeRequest, BlobrayApplication};

#[test]
fn configure_accepts_a_bare_manifest_relative_to_the_caller() {
    const CHILD: &str = "BLOBRAY_TEST_BARE_CONFIGURE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let report = blobray::configure_project(
            Path::new("vendor-project.toml"),
            blobray::ProjectConfigureRequest {
                ecosystem_packs: Some(vec!["ecosystem.toml".into()]),
                check: false,
            },
        )
        .unwrap();
        assert_eq!(report.ecosystem_packs, ["portable"]);
        assert!(Path::new(&report.manifest).is_absolute());
        return;
    }
    let root = std::env::temp_dir().join(format!("blobray-bare-configure-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let target =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic-project/target.toml");
    std::fs::write(
        root.join("vendor-project.toml"),
        format!(
            "schema = 4\nid = \"portable\"\ntarget-spec = {:?}\n",
            target.to_str().unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("ecosystem.toml"),
        "schema = 3\nid = \"portable\"\nknowledge-packs = []\n",
    )
    .unwrap();
    // Isolate cwd in a child process so parallel tests keep their own context.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "configure_accepts_a_bare_manifest_relative_to_the_caller",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn analyze_requires_a_unique_archive_symbol_and_honors_member_selection() {
    let hex = include_str!("fixtures/generic-e2e-rv32.hex")
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    let object = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|octet| u8::from_str_radix(std::str::from_utf8(octet).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    let mut archive = b"!<arch>\n".to_vec();
    for member in ["first.elf/", "second.elf/"] {
        writeln!(
            archive,
            "{member:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`",
            0,
            0,
            0,
            "100644",
            object.len()
        )
        .unwrap();
        archive.extend_from_slice(&object);
        if object.len() % 2 != 0 {
            archive.push(b'\n');
        }
    }
    let artifact = std::env::temp_dir().join(format!(
        "blobray-application-ambiguous-{}.a",
        std::process::id()
    ));
    std::fs::write(&artifact, archive).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/generic-project/vendor-project.toml");
    let mut application = BlobrayApplication::open(&manifest).unwrap();
    let request = |member: Option<&str>| AnalyzeRequest {
        artifact: artifact.clone(),
        member: member.map(str::to_owned),
        symbol: "fixture_callback".to_owned(),
    };

    // Equal bytes do not make two independently selectable definitions unique.
    let error = application.analyze(request(None)).unwrap_err().to_string();
    assert!(error.contains("ambiguous"), "{error}");
    assert!(error.contains("first.elf"), "{error}");
    assert!(error.contains("second.elf"), "{error}");
    for member in ["first.elf", "second.elf"] {
        let report = application.analyze(request(Some(member))).unwrap();
        assert_eq!(report.symbol, "fixture_callback");
        assert!(report.exact);
    }
    let error = application
        .analyze(request(Some("absent.elf")))
        .unwrap_err()
        .to_string();
    assert!(error.contains("not found"), "{error}");
    // Successful explicit selection must not populate an unqualified cache key.
    assert!(application.analyze(request(None)).is_err());
    std::fs::remove_file(artifact).unwrap();
}
