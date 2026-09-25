use super::*;
use std::collections::BTreeSet;

pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub start: u64,
    pub rss: u64,
}
fn read(pid: u32) -> Result<Process> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(io)?;
    let (_, fields) = text
        .rsplit_once(") ")
        .ok_or_else(|| unavailable("malformed proc stat"))?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let number = |i: usize| {
        fields
            .get(i)
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| unavailable("malformed proc stat number"))
    };
    // SAFETY: sysconf has no pointer arguments or caller-owned mutable state.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page <= 0 {
        return Err(unavailable("cannot determine page size"));
    }
    Ok(Process {
        pid,
        parent: number(1)? as u32,
        start: number(19)?,
        rss: number(21)?.saturating_mul(page as u64),
    })
}
pub(super) fn identity(pid: u32) -> Result<OwnerIdentity> {
    Ok(OwnerIdentity {
        pid,
        start_ticks: read(pid)?.start,
        boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(io)?
            .trim()
            .into(),
    })
}
pub(super) fn alive(owner: &OwnerIdentity) -> Result<bool> {
    if !Path::new(&format!("/proc/{}", owner.pid)).exists() {
        return Ok(false);
    }
    match identity(owner.pid) {
        Ok(current) => Ok(current == *owner),
        Err(_) if !Path::new(&format!("/proc/{}", owner.pid)).exists() => Ok(false),
        Err(error) => Err(error),
    }
}
/// Live descendants of `root`. Follows the kernel's per-task `children` lists,
/// which reach every reparented orphan because the guard is a subreaper; this
/// reads a few files instead of every process in the system. Without
/// `CONFIG_PROC_CHILDREN` it falls back to a complete `/proc` scan.
pub(super) fn descendants(root: u32) -> Result<Vec<Process>> {
    if !Path::new(&format!("/proc/{root}/task/{root}/children")).exists() {
        return scan_descendants(root);
    }
    let mut result = Vec::new();
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        let Ok(tasks) = fs::read_dir(format!("/proc/{parent}/task")) else {
            if parent == root {
                return Err(unavailable("cannot list guard tasks"));
            }
            continue; // The descendant exited after it was listed.
        };
        for task in tasks {
            let task = task.map_err(io)?;
            let Ok(children) = fs::read_to_string(task.path().join("children")) else {
                continue; // The task exited after it was listed.
            };
            for child in children.split_ascii_whitespace() {
                let pid = child
                    .parse::<u32>()
                    .map_err(|_| unavailable("malformed proc children"))?;
                match read(pid) {
                    Ok(process) => {
                        result.push(process);
                        pending.push(pid);
                    }
                    Err(_) if !Path::new(&format!("/proc/{pid}")).exists() => (),
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(result)
}
fn scan_descendants(root: u32) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    for entry in fs::read_dir("/proc").map_err(io)? {
        let entry = entry.map_err(io)?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        match read(pid) {
            Ok(process) => processes.push(process),
            Err(_) if !entry.path().exists() => (),
            Err(error) => return Err(error),
        }
    }
    let mut selected = BTreeSet::from([root]);
    loop {
        let before = selected.len();
        for process in &processes {
            if selected.contains(&process.parent) {
                selected.insert(process.pid);
            }
        }
        if selected.len() == before {
            break;
        }
    }
    Ok(processes
        .into_iter()
        .filter(|p| p.pid != root && selected.contains(&p.pid))
        .collect())
}
pub(super) fn signal(process: &Process, signal: i32) {
    if read(process.pid).is_ok_and(|p| p.start == process.start) {
        // SAFETY: only a freshly identified descendant PID is signalled.
        unsafe {
            libc::kill(process.pid as i32, signal);
        }
    }
}
pub(super) fn current_cgroup() -> Result<PathBuf> {
    let text = fs::read_to_string("/proc/self/cgroup").map_err(io)?;
    let path = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| unavailable("cgroup v2 membership unavailable"))?;
    Ok(Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
}

/// Block until child `pid` exits or `timeout_ms` elapses, whichever is first.
/// Exit wakes the caller immediately; without pidfd support this sleeps.
pub(super) fn wait_exit(pid: u32, timeout_ms: u64) {
    let timeout = i32::try_from(timeout_ms).unwrap_or(i32::MAX);
    // SAFETY: pidfd_open takes a pid and zero flags and returns a new owned file
    // descriptor or -1; it does not access caller memory.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } as i32;
    if fd < 0 {
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms));
        return;
    }
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` is a valid, exclusively borrowed pollfd array of length
    // one for the duration of the call, and `fd` is owned and closed only here.
    unsafe {
        libc::poll(&mut descriptor, 1, timeout);
        libc::close(fd);
    }
}
