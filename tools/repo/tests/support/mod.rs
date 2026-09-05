#![allow(dead_code)]
use oer_xtask::{Context, process};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Fixture {
    pub temporary: tempfile::TempDir,
    pub context: Context,
    pub manifest: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("oer Rust граница ")
            .tempdir()
            .unwrap();
        fs::write(temporary.path().join("Cargo.toml"),"[workspace]\nmembers = [\"adapter\", \"helper\", \"driver/chips/test-radio\"]\nresolver = \"3\"\n").unwrap();
        let context = Context::new(temporary.path()).unwrap();
        let manifest = context.root.join("adapter/Cargo.toml");
        let fixture = Self {
            temporary,
            context,
            manifest,
        };
        fixture.package("adapter", "network-adapter-fixture", "");
        fixture.package("helper", "packet-helper", "");
        fixture.package("driver/chips/test-radio", "device-registers", "");
        fixture
    }
    pub fn root(&self) -> &Path {
        &self.context.root
    }
    pub fn write(&self, path: &str, contents: &str) {
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    pub fn package(&self, path: &str, name: &str, dependencies: &str) {
        self.write(&format!("{path}/Cargo.toml"),&format!("[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n{dependencies}"));
        self.write(&format!("{path}/src/lib.rs"), "");
    }
    pub fn metadata(&self) -> Value {
        process::capture(
            self.context
                .cargo()
                .args(["generate-lockfile", "--offline"]),
        )
        .unwrap();
        let output = process::capture(
            self.context
                .cargo()
                .args([
                    "metadata",
                    "--format-version",
                    "1",
                    "--locked",
                    "--offline",
                    "--manifest-path",
                ])
                .arg(&self.manifest),
        )
        .unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    }
    pub fn git(&self, args: &[&str]) {
        process::capture(self.context.command("git").args(args)).unwrap();
    }
}
