use crate::config::ProjectConfig;
use anyhow::Context;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

// ──────────────────────────────────────────────────────────────────────────────
// Public result type
// ──────────────────────────────────────────────────────────────────────────────

/// The outcome of a single `deploy()` call.
#[derive(Debug)]
pub enum DeployResult {
    /// `docker compose up -d` succeeded and at least one service image changed.
    Deployed,
    /// Images were already up-to-date; `compose up` was intentionally skipped.
    UpToDate,
    /// `docker compose up -d` failed.
    Failed(anyhow::Error),
}

// ──────────────────────────────────────────────────────────────────────────────
// Core deploy logic
// ──────────────────────────────────────────────────────────────────────────────

/// Run `docker compose up -d` (optionally preceded by `docker compose pull`)
/// for the given project.
///
/// When `pull_before_up` is enabled (the default) this function also checks
/// whether any service image actually changed during the pull.  If nothing
/// changed – i.e. every image was already at the latest digest – the `up`
/// step is skipped and [`DeployResult::UpToDate`] is returned.  This avoids
/// unnecessary container restarts and prevents double-updates when both the
/// webhook and watcher are running against overlapping images.
///
/// All stdout/stderr from docker is captured and emitted as structured log
/// fields so it ends up in the JSON log stream.
pub async fn deploy(project: &ProjectConfig) -> DeployResult {
    let compose_dir = &project.compose_dir;

    if project.pull_before_up {
        info!(
            project = %project.name,
            compose_dir = %compose_dir.display(),
            "running docker compose pull"
        );

        // Snapshot image IDs before pulling so we can detect changes.
        let ids_before = image_ids(project).await;

        let pull_output = match build_compose_command("pull", project)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn docker compose pull")
            .and_then(|c| {
                // We need to block-wait; use a synchronous approach via async.
                // Return the future; the caller is async.
                Ok(c)
            }) {
            Ok(child) => match child.wait_with_output().await.context("docker compose pull did not complete") {
                Ok(o) => o,
                Err(e) => {
                    warn!(project = %project.name, err = %e, "docker compose pull failed to run");
                    return DeployResult::Failed(e);
                }
            },
            Err(e) => {
                warn!(project = %project.name, err = %e, "docker compose pull spawn failed");
                return DeployResult::Failed(e);
            }
        };

        log_output("docker compose pull", &project.name, &pull_output);

        if !pull_output.status.success() {
            warn!(
                project = %project.name,
                exit_code = ?pull_output.status.code(),
                "docker compose pull failed; continuing to 'up' anyway"
            );
        } else {
            // Pull succeeded: compare image IDs after the pull.
            let ids_after = image_ids(project).await;

            if !ids_before.is_empty() && ids_before == ids_after {
                info!(
                    project = %project.name,
                    "all service images already up-to-date; skipping compose up"
                );
                return DeployResult::UpToDate;
            }
        }
    }

    // ── docker compose up -d ────────────────────────────────────────────────
    info!(
        project = %project.name,
        compose_dir = %compose_dir.display(),
        "running docker compose up -d"
    );

    let output = match build_compose_command("up", project)
        .arg("-d")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn docker compose up")
    {
        Ok(child) => match child.wait_with_output().await.context("docker compose up did not complete") {
            Ok(o) => o,
            Err(e) => return DeployResult::Failed(e),
        },
        Err(e) => return DeployResult::Failed(e),
    };

    log_output("docker compose up", &project.name, &output);

    if output.status.success() {
        info!(project = %project.name, "deployment successful");
        DeployResult::Deployed
    } else {
        let code = output.status.code().unwrap_or(-1);
        error!(
            project = %project.name,
            exit_code = code,
            "docker compose up failed"
        );
        DeployResult::Failed(anyhow::anyhow!(
            "docker compose up -d failed for project '{}' (exit {})",
            project.name,
            code
        ))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Collect image IDs for all services in the compose project.
///
/// Runs `docker compose images -q` which prints one image ID per service line.
/// Returns a sorted `Vec<String>` so that order-independent comparison works.
/// Returns an empty vec on any error (safe fallback: assume changed).
async fn image_ids(project: &ProjectConfig) -> Vec<String> {
    let output = match build_compose_command("images", project)
        .arg("-q")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => match child.wait_with_output().await {
            Ok(o) => o,
            Err(_) => return vec![],
        },
        Err(_) => return vec![],
    };

    if !output.status.success() {
        return vec![];
    }

    let mut ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    ids.sort();
    ids
}

/// Build a `docker compose <subcommand>` [`Command`] with the correct
/// working directory, compose file override, and extra environment variables.
fn build_compose_command(subcommand: &str, project: &ProjectConfig) -> Command {
    let mut cmd = Command::new("docker");
    cmd.arg("compose");

    if let Some(ref file) = project.compose_file {
        cmd.arg("-f").arg(file);
    }

    cmd.arg(subcommand);
    cmd.current_dir(&project.compose_dir);

    for (k, v) in &project.env {
        cmd.env(k, v);
    }

    cmd
}

/// Emit captured stdout/stderr as structured log lines.
fn log_output(label: &str, project: &str, output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stdout.trim().is_empty() {
        info!(
            project = %project,
            command = %label,
            output = %stdout.trim(),
            "stdout"
        );
    }
    if !stderr.trim().is_empty() {
        // Docker compose writes progress to stderr; treat as info unless
        // the process actually failed (caller decides).
        info!(
            project = %project,
            command = %label,
            output = %stderr.trim(),
            "stderr"
        );
    }
}
