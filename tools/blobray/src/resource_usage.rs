//! Process-resource helpers for boundaries between large analysis phases.
//!
//! Rust correctly drops a completed profile, but glibc may retain its freed
//! heap pages for later allocations. Artifact-wide IR consists of many small
//! objects, so running several profiles sequentially can otherwise preserve
//! the first profile's allocator high-water mark. The helper below asks glibc
//! to return fully free pages to the operating system. It is an optimization
//! only: unsupported targets keep normal allocator behaviour.

/// Release unused allocator pages after a large, fully dropped analysis phase.
pub(crate) fn release_unused_memory(phase: &'static str) {
    let before_kib = resident_set_kib();
    let released = trim_allocator();
    let after_kib = resident_set_kib();
    tracing::debug!(
        phase,
        released,
        rss_before_kib = before_kib,
        rss_after_kib = after_kib,
        "released unused analysis memory"
    );
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_allocator() -> bool {
    // SAFETY: malloc_trim(0) has no pointer arguments and only asks the glibc
    // allocator used by this process to release completely free heap pages.
    unsafe { libc::malloc_trim(0) != 0 }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim_allocator() -> bool {
    false
}

#[cfg(target_os = "linux")]
pub(crate) fn resident_set_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_resident_set_kib(&status)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn resident_set_kib() -> Option<u64> {
    None
}

#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
fn parse_resident_set_kib(status: &str) -> Option<u64> {
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let mut fields = line.split_ascii_whitespace();
    (fields.next()? == "VmRSS:").then_some(())?;
    let value = fields.next()?.parse::<u64>().ok()?;
    match fields.next()? {
        "kB" => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_resident_set_kib;

    #[test]
    fn parses_linux_resident_set_size() {
        assert_eq!(
            parse_resident_set_kib("Name:\tblobray\nVmRSS:\t  123456 kB\nThreads:\t1\n"),
            Some(123_456)
        );
    }

    #[test]
    fn rejects_missing_or_unexpected_units() {
        assert_eq!(parse_resident_set_kib("Name:\tblobray\n"), None);
        assert_eq!(parse_resident_set_kib("VmRSS:\t12 MB\n"), None);
    }
}
