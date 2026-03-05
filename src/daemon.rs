use anyhow::Context;
use nix::{
    sys::stat,
    unistd::{fork, setsid, ForkResult},
};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::Path};
use tracing::info;

/// Fork into the background and write a PID file.
///
/// Returns `Ok(true)` in the child process (the daemon), `Ok(false)` if
/// we are the parent that should exit cleanly.
pub fn daemonize(pid_file: &Path, log_dir: &Path) -> anyhow::Result<bool> {
    // Ensure log directory exists.
    fs::create_dir_all(log_dir).with_context(|| format!("create log_dir {}", log_dir.display()))?;

    // Ensure PID file directory exists.
    if let Some(parent) = pid_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create pid_file dir {}", parent.display()))?;
    }

    // SAFETY: fork() is safe in a single-threaded context at startup.
    match unsafe { fork() }.context("fork() failed")? {
        ForkResult::Parent { .. } => {
            // Parent exits cleanly; the shell gets its prompt back.
            return Ok(false);
        }
        ForkResult::Child => {}
    }

    // Create a new session so the daemon is detached from the terminal.
    setsid().context("setsid() failed")?;

    // Set umask.
    stat::umask(stat::Mode::from_bits_truncate(0o022));

    // Write PID file.
    write_pid_file(pid_file)?;

    Ok(true)
}

/// Write the current process PID to `path`.
pub fn write_pid_file(path: &Path) -> anyhow::Result<()> {
    let pid = std::process::id();
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o644)
        .open(path)
        .with_context(|| format!("open pid file {}", path.display()))?
        .write_all(format!("{}\n", pid).as_bytes())
        .with_context(|| format!("write pid file {}", path.display()))?;

    info!(pid = pid, path = %path.display(), "wrote PID file");
    Ok(())
}

/// Remove the PID file on clean shutdown.
pub fn remove_pid_file(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}
