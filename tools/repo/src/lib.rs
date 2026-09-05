//! Repository orchestration; policies remain separate from Cargo/process mechanics.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

pub mod cargo;
pub mod checks;
pub mod graph;
pub mod paths;
pub mod process;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug)]
pub struct Context {
    pub root: PathBuf,
    pub cargo: OsString,
}

impl Context {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.join("Cargo.toml").is_file() {
            return Err(format!("repository manifest missing: {}", root.display()).into());
        }
        Ok(Self {
            root,
            cargo: std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        })
    }

    pub fn discover() -> Result<Self> {
        Self::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.root);
        command
    }

    pub fn cargo(&self) -> Command {
        self.command(&self.cargo)
    }
}
