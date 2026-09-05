use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Auto,
    Systemd,
    Watchdog,
}

struct Config {
    binary: PathBuf,
    args: Vec<OsString>,
    backend: Backend,
    report_usage: bool,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct Limits {
    memory_bytes: u64,
    deadline: Duration,
    grace: Duration,
    poll: Duration,
}

#[cfg(target_os = "linux")]
impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 1024 * 1024 * 1024,
            deadline: Duration::from_secs(15 * 60),
            grace: Duration::from_secs(10),
            poll: Duration::from_millis(100),
        }
    }
}

pub fn main() -> ExitCode {
    match Config::from_environment().and_then(run) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

impl Config {
    fn from_environment() -> Result<Self, String> {
        let binary = std::env::var_os("BLOBRAY_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(default_binary);
        let backend = match std::env::var("BLOBRAY_LIMIT_BACKEND").as_deref() {
            Ok("auto") | Err(std::env::VarError::NotPresent) => Backend::Auto,
            Ok("systemd") => Backend::Systemd,
            Ok("watchdog") => Backend::Watchdog,
            _ => return Err("BLOBRAY_LIMIT_BACKEND must be auto, systemd, or watchdog".into()),
        };
        let report_usage = match std::env::var("BLOBRAY_REPORT_USAGE").as_deref() {
            Ok("0") | Err(std::env::VarError::NotPresent) => false,
            Ok("1") => true,
            _ => return Err("BLOBRAY_REPORT_USAGE must be 0 or 1".into()),
        };
        Ok(Self {
            binary: selected_binary(&binary)?,
            args: std::env::args_os().skip(1).collect(),
            backend,
            report_usage,
        })
    }
}

fn default_binary() -> PathBuf {
    // Discover the Cargo workspace at build time, including extracted Blobray.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .ancestors()
        .find(|path| {
            std::fs::read_to_string(path.join("Cargo.toml"))
                .is_ok_and(|contents| contents.lines().any(|line| line.trim() == "[workspace]"))
        })
        .unwrap_or(manifest);
    root.join("target/blobray")
        .join(format!("blobray{}", std::env::consts::EXE_SUFFIX))
}

fn selected_binary(path: &Path) -> Result<PathBuf, String> {
    let error = || {
        format!(
            "optimized blobray binary is missing or not executable: {}; set BLOBRAY_BINARY to an executable built with --profile blobray",
            path.display()
        )
    };
    // Canonicalization prevents a bare relative filename from becoming a PATH lookup.
    let path = path.canonicalize().map_err(|_| error())?;
    let metadata = path.metadata().map_err(|_| error())?;
    if !metadata.is_file() {
        return Err(error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(error());
        }
    }
    Ok(path)
}

fn run(config: Config) -> Result<u8, String> {
    #[cfg(target_os = "linux")]
    {
        linux::run(config)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err("Unsupported: this platform has no backend enforcing Blobray's process-tree memory and runtime policy".into())
    }
}
