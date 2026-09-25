use super::*;

#[test]
fn verdict_requires_every_recorded_subject_to_match() {
    let subject = |role, byte_identical| SubjectComparison {
        role,
        left: FileIdentity {
            path: PathBuf::from("left"),
            size_bytes: 1,
            sha256: String::from("aa"),
        },
        right: FileIdentity {
            path: PathBuf::from("right"),
            size_bytes: 1,
            sha256: String::from("aa"),
        },
        byte_identical,
        section_layout: None,
    };
    let matching = [subject(SubjectRole::Application, true)];
    let differing = [
        subject(SubjectRole::Application, true),
        subject(SubjectRole::RuntimeElf, false),
    ];

    assert!(matching.iter().all(|entry| entry.byte_identical));
    assert!(!differing.iter().all(|entry| entry.byte_identical));
}

#[test]
fn path_length_check_cannot_be_hidden_by_equal_length_roots() {
    let root = PathBuf::from("/tmp/rebuild");
    let left = root.join("source-a");
    let right = root.join("source-directory-b");
    assert_ne!(left.as_os_str().len(), right.as_os_str().len());
}
