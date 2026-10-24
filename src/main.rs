mod compose;
mod config;
mod daemon;
mod harbor;
mod matcher;
mod metrics;
mod notify;
mod server;
mod swap;
mod watcher;

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::{path::PathBuf, sync::Arc};
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// ──────────────────────────────────────────────────────────────────────────────
// CLI definition
// ──────────────────────────────────────────────────────────────────────────────

/// updock – Harbor webhook daemon + Watchtower-style auto-updater.
#[derive(Parser, Debug)]
#[command(
    name = "updock",
    version,
    about = "Harbor webhook receiver and Watchtower-style container auto-updater"
)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "updock.yaml", env = "UPDOCK_CONFIG")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the webhook daemon (and watcher loop if enabled).
    Start {
        /// Override daemon.enabled from the config file.
        #[arg(long)]
        daemon: bool,
    },

    /// Stop a running daemon (sends SIGTERM via the PID file).
    Stop,

    /// Check the configuration file for errors and print the parsed result.
    Check,

    /// Print the current status (running / stopped).
    Status,
}

// ──────────────────────────────────────────────────────────────────────────────
// Entry point
// ──────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load config early so all subcommands can use it.
    let cfg = config::load(&cli.config)?;

    match cli.command {
        Commands::Check => cmd_check(&cfg),
        Commands::Status => cmd_status(&cfg),
        Commands::Stop => cmd_stop(&cfg),
        Commands::Start { daemon: daemonize_flag } => {
            cmd_start(cfg, daemonize_flag).await?;
        }
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Sub-commands
// ──────────────────────────────────────────────────────────────────────────────

fn cmd_check(cfg: &config::Config) {
    println!("Configuration is valid.\n");
    println!("Server bind       : {}", cfg.server.bind);
    println!("Webhook path      : {}", cfg.server.webhook_path);
    println!(
        "Secret            : {}",
        if cfg.server.secret.is_some() { "<set>" } else { "<not set>" }
    );
    println!("Daemon enabled    : {}", cfg.daemon.enabled);
    println!("PID file          : {}", cfg.daemon.pid_file.display());
    println!();
    println!("Watcher enabled   : {}", cfg.watcher.enabled);
    if cfg.watcher.enabled {
        if let Some(ref sched) = cfg.watcher.schedule {
            println!("Watcher schedule  : {}", sched);
        } else {
            println!("Watcher interval  : {}s", cfg.watcher.poll_interval);
        }
    }
    println!();
    println!("Projects ({}):", cfg.projects.len());
    for p in &cfg.projects {
        println!(
            "  [{}]  harbor_repository={:?}  tag_filter={:?}  compose_dir={}  pull={}",
            p.name,
            p.harbor_repository,
            p.tag_filter,
            p.compose_dir.display(),
            p.pull_before_up,
        );
    }
}

fn cmd_status(cfg: &config::Config) {
    let pid_file = &cfg.daemon.pid_file;
    if !pid_file.exists() {
        println!("stopped (no PID file at {})", pid_file.display());
        return;
    }

    match std::fs::read_to_string(pid_file) {
        Ok(content) => {
            let pid = content.trim();
            let alive = pid
                .parse::<u32>()
                .map(|p| {
                    nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(p as i32),
                        None,
                    )
                    .is_ok()
                })
                .unwrap_or(false);

            if alive {
                println!("running (PID {})", pid);
            } else {
                println!("stale PID file ({}) – process not found", pid_file.display());
            }
        }
        Err(e) => {
            eprintln!("cannot read PID file {}: {}", pid_file.display(), e);
        }
    }
}

fn cmd_stop(cfg: &config::Config) {
    let pid_file = &cfg.daemon.pid_file;
    if !pid_file.exists() {
        println!("no PID file found at {} – daemon may not be running", pid_file.display());
        return;
    }

    let content = match std::fs::read_to_string(pid_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot read PID file: {}", e);
            return;
        }
    };

    let pid: i32 = match content.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("invalid PID in {}", pid_file.display());
            return;
        }
    };

    match nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    ) {
        Ok(_) => println!("sent SIGTERM to PID {}", pid),
        Err(e) => eprintln!("failed to send SIGTERM to PID {}: {}", pid, e),
    }
}

async fn cmd_start(cfg: config::Config, daemonize_flag: bool) -> anyhow::Result<()> {
    let should_daemonize = daemonize_flag || cfg.daemon.enabled;

    // ── Daemonise if requested ───────────────────────────────────────────────
    if should_daemonize {
        let is_child = daemon::daemonize(&cfg.daemon.pid_file, &cfg.daemon.log_dir)
            .context("daemonize failed")?;
        if !is_child {
            std::process::exit(0);
        }
    }

    // ── Initialise structured JSON logging ──────────────────────────────────
    init_logging();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        bind    = %cfg.server.bind,
        path    = %cfg.server.webhook_path,
        daemon  = should_daemonize,
        watcher = cfg.watcher.enabled,
        "updock starting"
    );

    // Write PID file in non-daemon mode too (useful for stop command).
    if !should_daemonize {
        daemon::write_pid_file(&cfg.daemon.pid_file).ok();
    }

    // ── Build shared services ────────────────────────────────────────────────
    let metrics = metrics::Metrics::new();
    let notifier = notify::Notifier::new(cfg.notifications.clone());
    let cfg = Arc::new(cfg);

    // Channel for the HTTP /v1/update endpoint to signal an immediate scan.
    // Buffer of 1: a second request while one is already pending is a no-op.
    let (update_tx, update_rx) = tokio::sync::mpsc::channel::<()>(1);

    // ── Spawn watcher loop (if enabled) ─────────────────────────────────────
    let update_tx_opt: Option<tokio::sync::mpsc::Sender<()>> = if cfg.watcher.enabled {
        let watcher_cfg = Arc::new(cfg.watcher.clone());
        let m = Arc::clone(&metrics);
        let n = Arc::clone(&notifier);

        match watcher::Watcher::new(Arc::clone(&watcher_cfg), m, n) {
            Ok(w) => {
                let w = Arc::new(w);
                let run_once = watcher_cfg.run_once;
                let poll_interval = watcher_cfg.poll_interval;
                let schedule = watcher_cfg.schedule.clone();

                tokio::spawn(async move {
                    run_watcher_loop(w, run_once, poll_interval, schedule, update_rx).await;
                });
                Some(update_tx)
            }
            Err(e) => {
                tracing::error!(err = %e, "failed to connect to Docker – watcher disabled");
                None
            }
        }
    } else {
        None
    };

    // ── Build router ─────────────────────────────────────────────────────────
    let app = server::build_router(Arc::clone(&cfg), Arc::clone(&metrics), Arc::clone(&notifier), update_tx_opt);

    // ── Bind listener ────────────────────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind(&cfg.server.bind)
        .await
        .with_context(|| format!("cannot bind to {}", cfg.server.bind))?;

    info!(bind = %cfg.server.bind, "webhook server listening");

    // ── Graceful shutdown on SIGTERM / SIGINT ────────────────────────────────
    let pid_file = cfg.daemon.pid_file.clone();
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("received SIGINT, shutting down");
            }
            _ = wait_sigterm() => {
                info!("received SIGTERM, shutting down");
            }
        }
        daemon::remove_pid_file(&pid_file);
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("server error")?;

    info!("updock stopped");
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Watcher scheduling loop
// ──────────────────────────────────────────────────────────────────────────────

async fn run_watcher_loop(
    w: Arc<watcher::Watcher>,
    run_once: bool,
    poll_interval: u64,
    schedule: Option<String>,
    mut update_rx: tokio::sync::mpsc::Receiver<()>,
) {
    if run_once {
        info!("watcher: run-once mode");
        if let Err(e) = w.run_once().await {
            tracing::error!(err = %e, "watcher run-once failed");
        }
        return;
    }

    if let Some(ref cron_expr) = schedule {
        // Cron-based scheduling via tokio-cron-scheduler.
        use tokio_cron_scheduler::{Job, JobScheduler};

        let scheduler = match JobScheduler::new().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(err = %e, "failed to create cron scheduler");
                return;
            }
        };

        let w2 = Arc::clone(&w);
        let expr = cron_expr.clone();
        let job = Job::new_async(expr.as_str(), move |_uuid, _lock| {
            let wc = Arc::clone(&w2);
            Box::pin(async move {
                if let Err(e) = wc.run_once().await {
                    tracing::error!(err = %e, "watcher scan failed");
                }
            })
        });

        match job {
            Ok(j) => {
                if let Err(e) = scheduler.add(j).await {
                    tracing::error!(err = %e, "failed to add cron job");
                    return;
                }
            }
            Err(e) => {
                tracing::error!(err = %e, "failed to create cron job");
                return;
            }
        }

        if let Err(e) = scheduler.start().await {
            tracing::error!(err = %e, "cron scheduler start failed");
            return;
        }

        info!(schedule = %cron_expr, "watcher cron loop started");

        // In cron mode, handle manual triggers from /v1/update while the
        // scheduler runs its own ticks in the background.
        loop {
            if update_rx.recv().await.is_some() {
                info!("watcher: immediate scan triggered via /v1/update");
                let wc = Arc::clone(&w);
                tokio::spawn(async move {
                    if let Err(e) = wc.run_once().await {
                        tracing::error!(err = %e, "on-demand watcher scan failed");
                    }
                });
            } else {
                // Channel closed – no more triggers possible.
                std::future::pending::<()>().await;
            }
        }
    } else {
        // Simple interval-based loop.
        let interval_secs = if poll_interval == 0 { 86400 } else { poll_interval };
        info!(interval_secs, "watcher interval loop started");

        // Run immediately on startup, then on each tick or trigger.
        loop {
            if let Err(e) = w.run_once().await {
                tracing::error!(err = %e, "watcher scan failed");
            }
            // Wait for either a timer tick or an immediate-scan request.
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval_secs)) => {}
                Some(()) = update_rx.recv() => {
                    info!("watcher: immediate scan triggered via /v1/update");
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Logging initialisation
// ──────────────────────────────────────────────────────────────────────────────

fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json())
        .init();
}

// ──────────────────────────────────────────────────────────────────────────────
// Signal helpers
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
async fn wait_sigterm() {
    use tokio::signal::unix::{signal, SignalKind};
    if let Ok(mut stream) = signal(SignalKind::terminate()) {
        stream.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(unix))]
async fn wait_sigterm() {
    std::future::pending::<()>().await;
}
