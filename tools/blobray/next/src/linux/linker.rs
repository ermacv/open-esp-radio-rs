//! LLD 22 adapter. All output flows through bounded application sinks.
use super::*;
use blobray_application::{LinkInput, LinkInvocation, LinkOutput, LinkOutputSink, LinkerHost};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStringExt,
            process::{CommandExt, ExitStatusExt},
        },
    },
};

pub struct Lld22;
fn digest(path: &Path, control: &mut dyn RunControl) -> Result<ArtifactId> {
    let mut file = File::open(path).map_err(io)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; WORK_BLOCK];
    loop {
        control.checkpoint(1)?;
        let count = file.read(&mut bytes).map_err(io)?;
        if count == 0 {
            break;
        }
        control.bytes(count)?;
        hash.update(&bytes[..count]);
    }
    format!("{:x}", hash.finalize()).parse()
}
fn pipe() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: two writable descriptors, flags valid for Linux pipe2.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    // SAFETY: successful pipe2 transfers ownership of two distinct descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}
struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn drain(
    child: &mut ChildOwner,
    channels: &[(i32, LinkOutput)],
    sink: &mut dyn LinkOutputSink,
    control: &mut dyn RunControl,
) -> Result<()> {
    let mut descriptors: Vec<_> = channels
        .iter()
        .map(|(fd, _)| libc::pollfd {
            fd: *fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    for (fd, _) in channels {
        // SAFETY: descriptors are live pipe reads retained by this invocation.
        let flags = unsafe { libc::fcntl(*fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(*fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
    }
    let mut status = None;
    let mut bytes = [0; WORK_BLOCK];
    while status.is_none() || descriptors.iter().any(|d| d.fd >= 0) {
        control.checkpoint(1)?;
        // SAFETY: valid array of pollfd, bounded 20ms wait.
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                20,
            )
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(io(error));
            }
        }
        for (index, descriptor) in descriptors.iter_mut().enumerate() {
            if descriptor.fd < 0 || descriptor.revents == 0 {
                continue;
            }
            // One bounded read per channel gives cancellation and other channels a turn.
            // SAFETY: live pipe and writable bounded buffer.
            let count =
                unsafe { libc::read(descriptor.fd, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                descriptor.fd = -1;
            } else if count > 0 {
                sink.write(channels[index].1, &bytes[..count as usize], control)?;
            } else {
                let error = std::io::Error::last_os_error();
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    return Err(io(error));
                }
            }
        }
        if status.is_none() {
            status = child.0.try_wait().map_err(io)?;
        }
    }
    sink.exited(status.unwrap().code(), status.unwrap().signal());
    if !status.unwrap().success() {
        return Err(Error::new(
            ErrorCode::LinkFailed,
            format!("LLD exited as {}", status.unwrap()),
        ));
    }
    Ok(())
}
fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only setrlimit runs after fork; it takes a valid immutable value.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}
impl LinkerHost for Lld22 {
    fn identify(&self, path: &Path, control: &mut dyn RunControl) -> Result<LinkerIdentity> {
        let path = path.canonicalize().map_err(io)?;
        let before = digest(&path, control)?;
        let mut child = ChildOwner(command(&path).arg("--version").spawn().map_err(io)?);
        let stdout = child.0.stdout.take().unwrap();
        let stderr = child.0.stderr.take().unwrap();
        struct Version(Vec<u8>);
        impl LinkOutputSink for Version {
            fn write(
                &mut self,
                _: LinkOutput,
                bytes: &[u8],
                control: &mut dyn RunControl,
            ) -> Result<()> {
                control.bytes(bytes.len())?;
                if self.0.len() + bytes.len() > 4096 {
                    return Err(Error::new(
                        ErrorCode::Incompatible,
                        "linker version output exceeds 4 KiB",
                    ));
                }
                self.0.extend_from_slice(bytes);
                Ok(())
            }
        }
        let mut version = Version(Vec::new());
        drain(
            &mut child,
            &[
                (stdout.as_raw_fd(), LinkOutput::Elf),
                (stderr.as_raw_fd(), LinkOutput::Stderr),
            ],
            &mut version,
            control,
        )?;
        let version = String::from_utf8(version.0)
            .map_err(|_| Error::new(ErrorCode::Incompatible, "linker version is not UTF-8"))?;
        let version = version.trim();
        if !version.starts_with("LLD 22.") {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "this adapter requires explicit LLVM LLD 22",
            ));
        }
        if before != digest(&path, control)? {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "linker changed during identification",
            ));
        }
        Ok(LinkerIdentity {
            implementation: "lld-elf-22".into(),
            version: version.into(),
            executable: before,
        })
    }
    fn link(
        &self,
        request: &LinkInvocation<'_>,
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if digest(request.executable, control)? != request.identity.executable {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "linker changed before launch",
            ));
        }
        let (map_read, map_write) = pipe()?;
        let (extract_read, extract_write) = pipe()?;
        let mut command = command(request.executable);
        command
            .current_dir(request.directory)
            .args([
                "-m",
                "elf32lriscv",
                "--threads=1",
                "--gc-sections",
                "--no-relax",
                "--icf=none",
                "--emit-relocs",
                "--no-demangle",
                "--error-unresolved-symbols",
                "--no-undefined",
                "--build-id=none",
                "-o",
                "-",
            ])
            .arg(format!("--Map=/proc/self/fd/{}", map_write.as_raw_fd()))
            .arg(format!(
                "--why-extract=/proc/self/fd/{}",
                extract_write.as_raw_fd()
            ))
            .arg("--entry")
            .arg(OsString::from_vec(request.entry.into()))
            .arg("--undefined")
            .arg(OsString::from_vec(request.entry.into()))
            .arg("-T")
            .arg(&request.script);
        for root in &request.roots {
            command
                .arg("--undefined")
                .arg(OsString::from_vec(root.clone()));
        }
        command.args(&request.forced);
        for input in &request.inputs {
            match input {
                LinkInput::Object(path) => {
                    command.arg(path);
                }
                LinkInput::Archive(paths) => {
                    command.arg("--start-lib").args(paths).arg("--end-lib");
                }
            }
        }
        let inherited = [map_write.as_raw_fd(), extract_write.as_raw_fd()];
        // SAFETY: descriptors remain owned through spawn. Only fcntl runs after fork.
        unsafe {
            command.pre_exec(move || {
                for fd in inherited {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = ChildOwner(command.spawn().map_err(io)?);
        drop(map_write);
        drop(extract_write);
        let stdout = child.0.stdout.take().unwrap();
        let stderr = child.0.stderr.take().unwrap();
        drain(
            &mut child,
            &[
                (stdout.as_raw_fd(), LinkOutput::Elf),
                (map_read.as_raw_fd(), LinkOutput::Map),
                (extract_read.as_raw_fd(), LinkOutput::Extraction),
                (stderr.as_raw_fd(), LinkOutput::Stderr),
            ],
            sink,
            control,
        )?;
        if digest(request.executable, control)? != request.identity.executable {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "linker changed during execution",
            ));
        }
        Ok(())
    }
}
