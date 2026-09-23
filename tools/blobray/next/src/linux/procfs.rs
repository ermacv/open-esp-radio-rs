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
pub(super) fn descendants(root: u32) -> Result<Vec<Process>> {
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
