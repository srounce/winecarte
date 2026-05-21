use anyhow::Context;
use async_trait::async_trait;
use clap::Parser;
use std::{
    env,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::{process, signal::unix::{SignalKind, signal}, time};
use tokio_util::sync::CancellationToken;

mod handlers;
use handlers::{RunnerState, get_handler};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(short = 'i', long)]
    appid: Option<String>,

    #[arg(required(true), last(false), trailing_var_arg(true))]
    startup_command: Vec<String>,
}

#[derive(Error, Debug)]
enum StartupError {
    #[error("No startup command provided")]
    MissingStartupCommand,

    #[error("No Steam AppId provided")]
    MissingAppId,
    #[error("Unsupported AppId provided: {0}")]
    UnsupportedAppId(String),

    #[error("No STEAM_COMPAT_DATA_PATH provided")]
    MissingCompatDataPath,
    #[error("Invalid STEAM_COMPAT_DATA_PATH provided")]
    InvalidCompatDataPath,

    #[error("No STEAM_COMPAT_TOOL_PATHS provided")]
    MissingCompatToolPath,
    #[error("Invalid STEAM_COMPAT_TOOL_PATHS provided")]
    InvalidCompatToolPath,
}

struct AppContext {
    compat_data_path: PathBuf,
    steam_linux_runtime_path: PathBuf,
    handler_appid: String,
    steam_appid: String,
}

pub(crate) fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;

    env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(program);
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_env("WINECARTE_LOG_LEVEL")
        .format_level(true)
        .format_module_path(true)
        .format_target(true)
        .try_init()?;

    let args = Args::parse();

    let steam_appid = std::env::var("SteamAppId").map_err(|_| StartupError::MissingAppId)?;
    let handler_appid = args.appid.clone().unwrap_or_else(|| steam_appid.clone());

    let compat_data_path = std::env::var("STEAM_COMPAT_DATA_PATH")
        .map_err(|_| StartupError::MissingCompatDataPath)
        .map(PathBuf::from)
        .map(|mut path| {
            path.push("pfx");
            path
        })
        .and_then(|path| match path.exists() {
            true => Ok(path),
            false => Err(StartupError::InvalidCompatDataPath),
        })?;

    let (compat_tool_path, steam_linux_runtime_path) = std::env::var("STEAM_COMPAT_TOOL_PATHS")
        .map_err(|_| StartupError::MissingCompatToolPath)
        .and_then(|value| {
            value
                .split_once(':')
                .map(|(first, second)| (PathBuf::from(first), PathBuf::from(second)))
                .ok_or(StartupError::InvalidCompatToolPath)
        })
        .and_then(|(compat_tool_path, steam_linux_runtime_path)| {
            if !compat_tool_path.exists() || !steam_linux_runtime_path.exists() {
                return Err(StartupError::InvalidCompatToolPath);
            }

            Ok((compat_tool_path, steam_linux_runtime_path))
        })?;

    log::info!("Wrapping handler AppId: {handler_appid}");
    log::info!("Steam AppId: {steam_appid}");
    log::info!(
        "Proton path: {}",
        compat_tool_path.to_str().unwrap_or_default()
    );
    log::info!(
        "Prefix path: {}",
        compat_data_path.to_str().unwrap_or_default()
    );
    log::info!(
        "Steam Linux Runtime path: {}",
        steam_linux_runtime_path.to_str().unwrap_or_default()
    );

    let context = AppContext {
        compat_data_path,
        steam_linux_runtime_path,
        handler_appid,
        steam_appid,
    };

    let shutdown = CancellationToken::new();

    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm = signal(SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
            let mut sigint = signal(SignalKind::interrupt())
                .expect("failed to register SIGINT handler");
            tokio::select! {
                _ = sigterm.recv() => {},
                _ = sigint.recv() => {},
            }
            log::info!("shutdown signal received");
            shutdown.cancel();
        });
    }

    let mut handler = get_handler(&context.handler_appid)?;
    handler.setup(&context)?;

    if args.startup_command.is_empty() {
        return Err(StartupError::MissingStartupCommand.into());
    }

    let (startup_command, startup_args) = args.startup_command.split_at(1);

    let mut command = process::Command::new(startup_command.first().unwrap());
    command
        .args(startup_args)
        .env("STEAM_COMPAT_LAUNCHER_SERVICE", "proton")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    log::info!("Running: {command:?}");
    let mut child_process = command.spawn().with_context(|| "Child command failure")?;
    log::info!("spawned launcher child for app {}", context.handler_appid);
    run_handler_loop(&context, &mut *handler, &mut child_process, shutdown).await
}

async fn run_handler_loop(
    context: &AppContext,
    handler: &mut dyn AppHandler,
    child_process: &mut process::Child,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let mut state = RunnerState::PreStart;
    let mut helper_started = false;
    let mut failure = None;
    let startup_deadline = Instant::now() + handler.startup_timeout();
    let mut launcher_exit_status = None;

    loop {
        match state {
            RunnerState::PreStart => {
                log::info!("runner state=PreStart for app {}", context.handler_appid);
                state = RunnerState::WaitingForGame;
                continue;
            }
            RunnerState::WaitingForGame => {
                log::debug!(
                    "runner state=WaitingForGame for app {}",
                    context.handler_appid
                );
                tokio::select! {
                    _ = time::sleep(Duration::from_secs(1)) => {},
                    _ = shutdown.cancelled() => {
                        state = RunnerState::Completed;
                        continue;
                    }
                    status = child_process.wait(), if launcher_exit_status.is_none() => {
                        launcher_exit_status = Some(status?);
                        log::info!("launcher exited for app {}; checking for game process", context.handler_appid);
                        if handler.probe_game_process(context)? {
                            log::info!("detected game startup for app {}", context.handler_appid);
                            handler.on_start(context)?;
                            helper_started = true;
                            state = RunnerState::Running;
                        } else {
                            state = RunnerState::CleanUp;
                        }
                        continue;
                    }
                }

                if handler.probe_game_process(context)? {
                    log::info!("detected game startup for app {}", context.handler_appid);
                    handler.on_start(context)?;
                    helper_started = true;
                    state = RunnerState::Running;
                    continue;
                }

                if Instant::now() >= startup_deadline {
                    failure = Some(anyhow::anyhow!(
                        "timed out waiting for game startup for app {}",
                        context.handler_appid
                    ));
                    state = RunnerState::Failed;
                    continue;
                }
            }
            RunnerState::Running => {
                log::debug!("runner state=Running for app {}", context.handler_appid);
                tokio::select! {
                    _ = time::sleep(Duration::from_secs(1)) => {},
                    _ = shutdown.cancelled() => {
                        state = RunnerState::CleanUp;
                        continue;
                    }
                }

                if handler.probe_game_process(context)? {
                    continue;
                }

                log::info!("detected game exit for app {}", context.handler_appid);
                state = RunnerState::CleanUp;
            }
            RunnerState::CleanUp => {
                log::info!("runner state=CleanUp for app {}", context.handler_appid);
                if helper_started {
                    handler.cleanup(context).await?;
                    helper_started = false;
                }

                state = RunnerState::Completed;
            }
            RunnerState::Completed => {
                log::info!("runner state=Completed for app {}", context.handler_appid);
                return Ok(());
            }
            RunnerState::Failed => {
                log::info!("runner state=Failed for app {}", context.handler_appid);
                return Err(failure
                    .take()
                    .unwrap_or_else(|| anyhow::anyhow!("runner entered failed state")));
            }
        }
    }
}

#[async_trait(?Send)]
trait AppHandler {
    fn setup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn on_start(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    async fn cleanup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn startup_timeout(&self) -> Duration {
        Duration::from_secs(120)
    }

    fn probe_game_process(&mut self, _context: &AppContext) -> anyhow::Result<bool> {
        Ok(false)
    }
}
