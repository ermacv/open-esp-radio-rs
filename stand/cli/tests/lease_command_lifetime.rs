//! External commands need only inherited lifetime descriptors, not broker APIs.
#![cfg(target_os = "linux")]

use std::{
    fs,
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use oer_device_lock::{Busy, DeviceAccess, DeviceId, Holder};
use oer_process::lock::LockBroker;

const TEST: &str = "external_io_retains_every_board_after_the_lease_owner_is_killed";
const ROLE: &str = "OER_LEASE_TEST_ROLE";
const DIRECTORY: &str = "OER_LEASE_TEST_DIRECTORY";
const BOARDS: [&str; 2] = ["00:11:22:33:44:66", "00:11:22:33:44:77"];

struct Owner(Child);

struct WriterCleanup(std::path::PathBuf);

impl Drop for WriterCleanup {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join("finish"), "finish");
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn wait_until(description: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{description}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn fixture(command: &mut Command, directory: &Path) {
    command
        .env(DIRECTORY, directory)
        .env("XDG_RUNTIME_DIR", directory)
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .env("XDG_DATA_HOME", directory.join("data"))
        .env(
            oer_stand_file::paths::STAND_FILE_ENV,
            directory.join("stand.toml"),
        )
        .env(
            oer_stand_file::paths::ARBITER_ENV,
            directory.join("arbiter"),
        )
        .stdout(Stdio::null());
}

fn contender(directory: &Path, role: &str) {
    let mut command = oer_process::command(std::env::current_exe().unwrap());
    fixture(&mut command, directory);
    command.args(["--exact", TEST]).env(ROLE, role);
    assert!(command.status().unwrap().success(), "{role}");
}

#[test]
fn external_io_retains_every_board_after_the_lease_owner_is_killed() {
    if let Ok(role) = std::env::var(ROLE) {
        let directory = std::path::PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
        match role.as_str() {
            "writer" => {
                // Model a third-party flasher: no Context or broker calls.
                // The guardian first sends SIGTERM after owner loss; keep
                // writing during that grace to expose early lock release.
                // SAFETY: install SIG_IGN for this single fixture process.
                unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
                let mut io = fs::File::create(directory.join("io")).unwrap();
                fs::write(directory.join("started"), "ready").unwrap();
                let deadline = Instant::now() + Duration::from_secs(15);
                while !directory.join("finish").exists() {
                    assert!(Instant::now() < deadline, "writer was not released");
                    io.write_all(b"external flash still running\n").unwrap();
                    thread::sleep(Duration::from_millis(10));
                }
                drop(io);
                fs::write(directory.join("closed"), "closed").unwrap();
            }
            "busy" | "free" => {
                assert_eq!(
                    oer_process::Context::current().unwrap(),
                    &Default::default()
                );
                for mac in BOARDS {
                    let id: DeviceId = mac.parse().unwrap();
                    let acquire = || DeviceAccess::try_acquire(&id, "unrelated contender").unwrap();
                    if role == "busy" {
                        assert!(matches!(acquire(), Err(Busy::Held(_))));
                    } else {
                        wait_until("board retained after external I/O ended", || {
                            acquire().is_ok()
                        });
                    }
                }
            }
            _ => panic!("unknown fixture role"),
        }
        return;
    }

    // Both explicit board claims and a whole-stand claim pin all selected boards.
    for claims in [
        vec!["--board", "one", "--board", "two", "--air", "none"],
        vec!["--stand"],
    ] {
        let directory = tempfile::Builder::new()
            .prefix(&"x".repeat(120))
            .tempdir()
            .unwrap();
        let _writer_cleanup = WriterCleanup(directory.path().to_owned());
        fs::write(
            directory.path().join("stand.toml"),
            "schema = 1\n[stand]\nid = 'test'\nair = 'exclusive'\n\
             [[board]]\nid = 'one'\nusb-serial = '00:11:22:33:44:66'\nchip = 'esp32c5'\n\
             radios = ['ble']\nroles = ['dut']\nreset = ['jtag']\n\
             [[board]]\nid = 'two'\nusb-serial = '00:11:22:33:44:77'\nchip = 'esp32c5'\n\
             radios = ['ble']\nroles = ['dut']\nreset = ['jtag']\n",
        )
        .unwrap();
        fs::set_permissions(
            directory.path().join("stand.toml"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let mut lease = oer_process::command(env!("CARGO_BIN_EXE_oer-stand"));
        fixture(&mut lease, directory.path());
        lease
            .args([
                "--root",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
                "--owner",
                "lifetime-test",
                "lease",
            ])
            .args(claims)
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", TEST])
            .env(ROLE, "writer");
        let mut owner = Owner(lease.spawn().unwrap());
        wait_until("leased writer did not start", || {
            directory.path().join("started").exists()
        });
        contender(directory.path(), "busy");
        owner.0.kill().unwrap();
        assert!(!owner.0.wait().unwrap().success());
        let locks = directory.path().join("open-esp-radio/devices");
        wait_until("owner loss did not close broker admission", || {
            BOARDS.iter().all(|mac| {
                let lock = locks.join(format!("{}.lock", mac.replace(':', "")));
                let holder: Holder =
                    serde_json::from_reader(fs::File::open(&lock).unwrap()).unwrap();
                LockBroker::operation(&lock, &holder.token).is_err()
            })
        });
        let before = fs::metadata(directory.path().join("io")).unwrap().len();
        wait_until("external I/O stopped when its owner died", || {
            fs::metadata(directory.path().join("io")).unwrap().len() > before
        });
        contender(directory.path(), "busy");
        fs::write(directory.path().join("finish"), "finish").unwrap();
        wait_until("writer did not close I/O", || {
            directory.path().join("closed").exists()
        });
        contender(directory.path(), "free");
    }
}
