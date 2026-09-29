use anyhow::Context;
use clap::Parser;
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::{
    process,
    signal::unix::{SignalKind, signal},
    time,
};
use tokio_util::sync::CancellationToken;
use winecarte_core::{
    games::{self, Game},
    launch::{self, SteamSession},
    procscan,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

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

impl AppContext {
    fn session(&self) -> SteamSession<'_> {
        SteamSession {
            compat_data_path: &self.compat_data_path,
            steam_linux_runtime_path: &self.steam_linux_runtime_path,
            steam_appid: &self.steam_appid,
        }
    }
}

enum RunnerState {
    /// Game command launched, waiting for the real game process to appear.
    WaitingForGame,
    /// Game process has been seen and is still alive.
    Running,
    /// Game process has exited, and winecarte-run is shutting down wine2linux.
    CleanUp,
    /// Cleanup is done and winecarte-run is exiting or has exited.
    Completed,
    /// Terminal failure state for launch timeout, launch error, or helper cleanup failure.
    Failed,
}

struct GameRunner {
    game: &'static Game,
    wine2linux: Option<process::Child>,
    /// Game exe plus its launchers: the session counts as alive while any of
    /// them runs, so a launcher that outlives the game does not cut it short.
    alive_markers: Vec<&'static str>,
}

impl GameRunner {
    fn new(game: &'static Game) -> Self {
        Self {
            game,
            wine2linux: None,
            alive_markers: game
                .process_names
                .iter()
                .chain(game.launcher_names)
                .copied()
                .collect(),
        }
    }

    fn on_start(&mut self, context: &AppContext) -> anyhow::Result<()> {
        let wine2linux_exe = launch::resolve_wine2linux_exe(
            env::var_os("WINECARTE_WINE2LINUX_EXE").map(Into::into),
        )?;
        log::info!("using wine2linux: {}", wine2linux_exe.display());
        let child = launch::launch_via_launcher_service(
            &context.session(),
            &wine2linux_exe,
            self.game.from_wine_args,
        )?;
        self.wine2linux = Some(child);
        Ok(())
    }

    async fn cleanup(&mut self) {
        if let Some(mut child) = self.wine2linux.take() {
            launch::stop_wine2linux(&mut child).await;
        }
    }

    fn game_is_alive(&self) -> anyhow::Result<bool> {
        procscan::game_is_alive(Path::new("/proc"), &self.alive_markers)
            .context("failed to read /proc")
    }
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

    let steam_appid = env::var("SteamAppId").map_err(|_| StartupError::MissingAppId)?;
    let handler_appid = args.appid.clone().unwrap_or_else(|| steam_appid.clone());

    let compat_data_path = env::var("STEAM_COMPAT_DATA_PATH")
        .map_err(|_| StartupError::MissingCompatDataPath)
        .map(|p| PathBuf::from(p).join("pfx"))
        .and_then(|path| match path.exists() {
            true => Ok(path),
            false => Err(StartupError::InvalidCompatDataPath),
        })?;

    let (compat_tool_path, steam_linux_runtime_path) = env::var("STEAM_COMPAT_TOOL_PATHS")
        .map_err(|_| StartupError::MissingCompatToolPath)
        .and_then(|value| {
            SteamSession::split_tool_paths(&value).ok_or(StartupError::InvalidCompatToolPath)
        })
        .and_then(|(tool, runtime)| {
            if !tool.exists() || !runtime.exists() {
                return Err(StartupError::InvalidCompatToolPath);
            }
            Ok((tool, runtime))
        })?;

    log::info!("Wrapping handler AppId: {handler_appid}");
    log::info!("Steam AppId: {steam_appid}");
    log::info!("Proton path: {}", compat_tool_path.display());
    log::info!("Prefix path: {}", compat_data_path.display());
    log::info!(
        "Steam Linux Runtime path: {}",
        steam_linux_runtime_path.display()
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
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");
            tokio::select! {
                _ = sigterm.recv() => {},
                _ = sigint.recv() => {},
            }
            log::info!("shutdown signal received");
            shutdown.cancel();
        });
    }

    let game = games::by_appid(&context.handler_appid)
        .ok_or_else(|| StartupError::UnsupportedAppId(context.handler_appid.clone()))?;
    let mut runner = GameRunner::new(game);

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
    run_handler_loop(&context, &mut runner, &mut child_process, shutdown).await
}

async fn run_handler_loop(
    context: &AppContext,
    runner: &mut GameRunner,
    child_process: &mut process::Child,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let mut state = RunnerState::WaitingForGame;
    let mut helper_started = false;
    let mut failure = None;
    let startup_deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut launcher_exit_status = None;
    let appid = &context.handler_appid;

    loop {
        match state {
            RunnerState::WaitingForGame => {
                log::debug!("runner state=WaitingForGame for app {appid}");
                tokio::select! {
                    _ = time::sleep(Duration::from_secs(1)) => {},
                    _ = shutdown.cancelled() => {
                        state = RunnerState::Completed;
                        continue;
                    }
                    status = child_process.wait(), if launcher_exit_status.is_none() => {
                        launcher_exit_status = Some(status?);
                        log::info!("launcher exited for app {appid}; checking for game process");
                        if runner.game_is_alive()? {
                            log::info!("detected game startup for app {appid}");
                            runner.on_start(context)?;
                            helper_started = true;
                            state = RunnerState::Running;
                        } else {
                            state = RunnerState::CleanUp;
                        }
                        continue;
                    }
                }

                if runner.game_is_alive()? {
                    log::info!("detected game startup for app {appid}");
                    runner.on_start(context)?;
                    helper_started = true;
                    state = RunnerState::Running;
                    continue;
                }

                if Instant::now() >= startup_deadline {
                    failure = Some(anyhow::anyhow!(
                        "timed out waiting for game startup for app {appid}"
                    ));
                    state = RunnerState::Failed;
                    continue;
                }
            }
            RunnerState::Running => {
                log::debug!("runner state=Running for app {appid}");
                tokio::select! {
                    _ = time::sleep(Duration::from_secs(1)) => {},
                    _ = shutdown.cancelled() => {
                        state = RunnerState::CleanUp;
                        continue;
                    }
                }

                if runner.game_is_alive()? {
                    continue;
                }

                log::info!("detected game exit for app {appid}");
                state = RunnerState::CleanUp;
            }
            RunnerState::CleanUp => {
                log::info!("runner state=CleanUp for app {appid}");
                if helper_started {
                    runner.cleanup().await;
                    helper_started = false;
                }

                state = RunnerState::Completed;
            }
            RunnerState::Completed => {
                log::info!("runner state=Completed for app {appid}");
                return Ok(());
            }
            RunnerState::Failed => {
                log::info!("runner state=Failed for app {appid}");
                return Err(failure
                    .take()
                    .unwrap_or_else(|| anyhow::anyhow!("runner entered failed state")));
            }
        }
    }
}
