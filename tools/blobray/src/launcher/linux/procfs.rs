use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub(super) struct Member {
    pub pid: i32,
    pub started: u64,
}

pub(super) struct Sample {
    pub members: Vec<Member>,
    pub rss_bytes: u64,
}

pub(super) trait Sampler {
    fn sample(&mut self, session: i32) -> io::Result<Sample>;
}

pub(super) struct Procfs {
    pub(super) root: PathBuf,
    page_bytes: u64,
}

impl Default for Procfs {
    fn default() -> Self {
        // SAFETY: sysconf has no pointer arguments or process side effects.
        let page_bytes = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Self {
            root: PathBuf::from("/proc"),
            page_bytes: u64::try_from(page_bytes).unwrap_or(0),
        }
    }
}

struct Stat {
    session: i32,
    started: u64,
    rss_pages: u64,
    zombie: bool,
}

fn stat(text: &str) -> io::Result<Stat> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed process resource record",
        )
    };
    // comm may itself contain spaces and parentheses.
    let fields: Vec<_> = text
        .rsplit_once(')')
        .ok_or_else(invalid)?
        .1
        .split_whitespace()
        .collect();
    Ok(Stat {
        session: fields
            .get(3)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?,
        started: fields
            .get(19)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?,
        rss_pages: fields
            .get(21)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?,
        zombie: matches!(fields.first().copied(), Some("Z" | "X")),
    })
}

impl Sampler for Procfs {
    fn sample(&mut self, session: i32) -> io::Result<Sample> {
        if self.page_bytes == 0 {
            return Err(io::Error::other("could not determine memory page size"));
        }
        let mut own_process_seen = false;
        let mut sample = Sample {
            members: Vec::new(),
            rss_bytes: 0,
        };
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            let text = match fs::read_to_string(entry.path().join("stat")) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let record = stat(&text)?;
            own_process_seen |= pid == std::process::id() as i32;
            if record.session == session && !record.zombie {
                sample.rss_bytes = sample
                    .rss_bytes
                    .checked_add(
                        record
                            .rss_pages
                            .checked_mul(self.page_bytes)
                            .ok_or_else(|| io::Error::other("RSS overflow"))?,
                    )
                    .ok_or_else(|| io::Error::other("session RSS overflow"))?;
                sample.members.push(Member {
                    pid,
                    started: record.started,
                });
            }
        }
        if !own_process_seen {
            return Err(io::Error::other(
                "process enumeration omitted the monitor itself",
            ));
        }
        Ok(sample)
    }
}

pub(super) fn is_same_process(member: Member) -> bool {
    fs::read_to_string(format!("/proc/{}/stat", member.pid))
        .ok()
        .and_then(|text| stat(&text).ok())
        .is_some_and(|record| record.started == member.started && !record.zombie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_resource_records_are_errors() {
        for value in ["", "not a process table", "1 (worker) R 1 1 1"] {
            assert!(stat(value).is_err());
        }
    }

    #[test]
    fn process_names_can_contain_parentheses_and_spaces() {
        let own = fs::read_to_string("/proc/self/stat").unwrap();
        let tail = own.rsplit_once(')').unwrap().1;
        let renamed = format!("1 (worker (nested) name){tail}");
        assert_eq!(stat(&own).unwrap().started, stat(&renamed).unwrap().started);
    }
}
