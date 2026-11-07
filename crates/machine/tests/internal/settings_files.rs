#![allow(clippy::expect_used)]
use super::*;
#[test]
fn prompt_creation_reports_failed_writes_and_tolerates_a_concurrent_creator() {
    let directory = tempfile::tempdir().expect("directory");
    let mut files = crate::files::AtomicFiles {
        create: |_| Err(std::io::Error::other("disk failure")),
        ..Default::default()
    };
    assert!(initialize_prompts_with(directory.path(), &files).is_err());
    files.create = |_| Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    initialize_prompts_with(directory.path(), &files).expect("concurrent create");
    #[cfg(unix)]
    {
        let path = directory.path().join("prompts/router/selection.md");
        std::os::unix::fs::symlink(&path, &path).expect("recursive link");
        assert!(initialize_prompts(directory.path()).is_err());
    }
}
