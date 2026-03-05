use crate::config::ProjectConfig;
use anyhow::Context;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

/// Run `docker compose up -d` (optionally preceded by `docker compose pull`)
/// for the given project.
///
/// All stdout / stderr from docker is captured and emitted as structured log
/// fields so it ends up in the JSON log stream.
pub async fn deploy(project: &ProjectConfig) -> anyhow::Result<()> {
    let compose_dir = &project.compose_dir;

    // ── optional: pull first ────────────────────────────────────────────────
    if project.pull_before_up {
        info!(
            project = %project.name,
            compose_dir = %compose_dir.display(),
            "running docker compose pull"
        );

        let status = build_compose_command("pull", project)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn docker compose pull")?
            .wait_with_output()
            .await
            .context("docker compose pull did not complete")?;

        log_output("docker compose pull", &project.name, &status);

        if !status.status.success() {
            warn!(
                project = %project.name,
                exit_code = ?status.status.code(),
                "docker compose pull failed; continuing to 'up' anyway"
            );
        }
    }

    // ── docker compose up -d ────────────────────────────────────────────────
    info!(
        project = %project.name,
        compose_dir = %compose_dir.display(),
        "running docker compose up -d"
    );

    let output = build_compose_command("up", project)
        .arg("-d")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn docker compose up")?
        .wait_with_output()
        .await
        .context("docker compose up did not complete")?;

    log_output("docker compose up", &project.name, &output);

    if output.status.success() {
        info!(
            project = %project.name,
            "deployment successful"
        );
        Ok(())
    } else {
        let code = output.status.code().unwrap_or(-1);
        error!(
            project = %project.name,
            exit_code = code,
            "docker compose up failed"
        );
        anyhow::bail!(
            "docker compose up -d failed for project '{}' (exit {})",
            project.name,
            code
        )
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

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
