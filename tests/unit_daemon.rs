/// Unit tests for daemon PID file operations.
use tempfile::TempDir;
use updock::daemon::{remove_pid_file, write_pid_file};

#[test]
fn write_pid_file_creates_file_with_current_pid() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.pid");

    write_pid_file(&path).expect("write_pid_file should succeed");

    assert!(path.exists(), "PID file should exist after write");

    let content = std::fs::read_to_string(&path).unwrap();
    let pid_in_file: u32 = content
        .trim()
        .parse()
        .expect("PID file should contain a number");
    let current_pid = std::process::id();
    assert_eq!(
        pid_in_file, current_pid,
        "PID in file should match current process"
    );
}

#[test]
fn write_pid_file_has_trailing_newline() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("newline.pid");
    write_pid_file(&path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.ends_with('\n'), "PID file should end with newline");
}

#[test]
fn write_pid_file_overwrites_existing_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("overwrite.pid");

    // Write some garbage first
    std::fs::write(&path, b"99999999\n").unwrap();

    write_pid_file(&path).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let pid_in_file: u32 = content.trim().parse().unwrap();
    assert_eq!(pid_in_file, std::process::id());
}

#[test]
fn remove_pid_file_deletes_existing_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("remove.pid");

    write_pid_file(&path).unwrap();
    assert!(path.exists());

    remove_pid_file(&path);
    assert!(!path.exists(), "PID file should be removed");
}

#[test]
fn remove_pid_file_is_noop_when_file_absent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nonexistent.pid");

    // Should not panic
    remove_pid_file(&path);
}

#[test]
fn write_pid_file_fails_if_parent_dir_missing() {
    let path = std::path::Path::new("/no/such/directory/updock.pid");
    let err = write_pid_file(path).unwrap_err();
    assert!(
        err.to_string().contains("pid file"),
        "error should mention pid file: {err}"
    );
}

#[test]
fn pid_file_content_is_only_digits_and_newline() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("only_digits.pid");
    write_pid_file(&path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    let trimmed = content.trim();
    assert!(
        trimmed.chars().all(|c| c.is_ascii_digit()),
        "PID file should contain only digits, got: {trimmed:?}"
    );
}
