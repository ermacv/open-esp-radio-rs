fn main() {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let parent = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .nth(1)
        .unwrap();
    let marker = std::path::PathBuf::from(std::env::var_os("OER_IMAGE_TEST_READY").unwrap());
    let pending = marker.with_extension("pending");
    std::fs::write(&pending, format!("{parent} {}", std::process::id())).unwrap();
    std::fs::rename(pending, marker).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(60));
}
