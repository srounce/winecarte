use anyhow::{Context, bail};
use clap::Parser;
use std::{
    env,
    path::PathBuf,
    process::{Command as StdCommand, Stdio},
    str, thread,
    time::Duration,
};
use thiserror::Error;
use tokio::process;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(short = 'i', long)]
    appid: Option<usize>,

    #[arg(required(true), last(false), trailing_var_arg(true))]
    startup_command: Vec<String>,
}

#[derive(Error, Debug)]
enum StartupError {
    #[error("Provided statup command was invalid")]
    InvalidStartupCommand,
    #[error("No startup command provided")]
    MissingStartupCommand,

    #[error("No Steam AppId provided")]
    MissingAppId,
    #[error("Invalid AppId provided")]
    InvalidAppId,
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
    appid: String,
}

fn get_handler(appid: &str) -> Result<Box<dyn AppHandler>, StartupError> {
    match appid {
        "2399420" => Ok(Box::new(LeMansUltimateHandler::default())),
        _ => Err(StartupError::UnsupportedAppId(appid.to_string())),
    }
}

fn print_env() {
    let mut vars = std::env::vars().collect::<Vec<_>>();
    vars.sort_by_key(|(key, _)| key.to_lowercase());
    for (key, value) in vars {
        println!("env: {key} => {value}");
    }
}

fn find_on_path(program: &str) -> Option<PathBuf> {
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

    let appid = std::env::var("SteamAppId").map_err(|_| StartupError::MissingAppId)?;

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

    log::info!("Wrapping AppId: {appid}");
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
        appid,
    };

    let mut handler = get_handler(&context.appid)?;
    handler.setup(&context)?;

    if args.startup_command.is_empty() {
        return Err(StartupError::MissingStartupCommand.into());
    }

    let (startup_command, startup_args) = args.startup_command.split_at(1);

    let mut command = process::Command::new(startup_command.first().unwrap());
    command
        .args(startup_args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    println!("Running: {command:?}");
    let mut child_process = command.spawn().with_context(|| "Child command failure")?;
    handler.on_start(&context)?;

    let status = child_process.wait().await?;

    println!("Exited with status {:?}", status.code().unwrap_or_default());
    handler.wait_for_game_exit(&context)?;

    handler.cleanup(&context)?;

    Ok(())
}

trait AppHandler {
    fn setup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn on_start(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn cleanup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn wait_for_game_exit(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct LeMansUltimateHandler {
    wine2linux_process: Option<process::Child>,
}

impl LeMansUltimateHandler {
    const GAME_PROCESS_MARKERS: [&'static str; 2] =
        ["Le Mans Ultimate.exe", "start_protected_game.exe"];

    fn resolve_runtime_launch_client(context: &AppContext) -> anyhow::Result<PathBuf> {
        if let Some(path) = std::env::var_os("WINECARTE_RUNTIME_LAUNCH_CLIENT") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Ok(path);
            }

            bail!(
                "WINECARTE_RUNTIME_LAUNCH_CLIENT points to a missing path: {}",
                path.display()
            );
        }

        let candidates = [
            context
                .steam_linux_runtime_path
                .join("pressure-vessel/bin/steam-runtime-launch-client"),
            context
                .steam_linux_runtime_path
                .join("ubuntu12_64/steam-runtime-launch-client"),
        ];

        for candidate in candidates {
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        bail!("could not find steam-runtime-launch-client; set WINECARTE_RUNTIME_LAUNCH_CLIENT")
    }

    fn resolve_wine2linux_exe() -> anyhow::Result<PathBuf> {
        if let Some(path) = find_on_path("wine2linux.exe") {
            return Ok(path);
        }

        if let Some(path) = std::env::var_os("WINECARTE_WINE2LINUX_EXE") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Ok(path);
            }

            bail!(
                "WINECARTE_WINE2LINUX_EXE points to a missing path: {}",
                path.display()
            );
        }

        bail!("could not find wine2linux.exe on PATH; set WINECARTE_WINE2LINUX_EXE")
    }

    fn game_is_alive(&self, context: &AppContext) -> anyhow::Result<bool> {
        let runtime_launch_client = Self::resolve_runtime_launch_client(context)?;
        let bus_name = format!("com.steampowered.App{}", context.appid);

        let output = StdCommand::new(runtime_launch_client)
            .arg("--bus-name")
            .arg(&bus_name)
            .arg("--")
            .arg("ps")
            .arg("-eo")
            .arg("args=")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .context("failed to query LMU process state inside pressure-vessel")?;

        if !output.status.success() {
            return Ok(false);
        }

        let stdout = str::from_utf8(&output.stdout).context("ps output was not valid UTF-8")?;

        Ok(stdout.lines().any(|line| {
            Self::GAME_PROCESS_MARKERS
                .iter()
                .any(|marker| line.contains(marker))
                && !line.contains("wine2linux.exe")
        }))
    }
}

impl AppHandler for LeMansUltimateHandler {
    fn on_start(&mut self, context: &AppContext) -> anyhow::Result<()> {
        let runtime_launch_client = Self::resolve_runtime_launch_client(context)?;
        let wine2linux_exe = Self::resolve_wine2linux_exe()?;
        let bus_name = format!("com.steampowered.App{}", context.appid);
        let retry_deadline = std::time::Instant::now() + Duration::from_secs(10);

        loop {
            let mut command = process::Command::new(&runtime_launch_client);
            command
                .arg("--bus-name")
                .arg(&bus_name)
                .arg("--directory=")
                .arg("--")
                .arg("wine")
                .arg(&wine2linux_exe)
                .arg("--map")
                .arg("LMU_Data")
                .arg("--event")
                .arg("LMU_Data_Event")
                .arg("--lmu-lock")
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .env("STEAM_COMPAT_DATA_PATH", &context.compat_data_path);

            eprintln!("Launching wine2linux helper: {command:?}");
            log::info!(
                "Launching wine2linux via steam-runtime-launch-client: {:?}",
                command
            );
            let mut child = command
                .spawn()
                .with_context(|| format!("failed to launch {}", wine2linux_exe.display()))?;

            thread::sleep(Duration::from_millis(250));
            if let Some(status) = child
                .try_wait()
                .context("failed to query wine2linux launcher status")?
            {
                if std::time::Instant::now() >= retry_deadline {
                    bail!(
                        "wine2linux launcher exited before the Steam command-launcher service became available; last status {:?}. \
Make sure Steam launch options include STEAM_COMPAT_LAUNCHER_SERVICE=proton",
                        status.code()
                    );
                }

                thread::sleep(Duration::from_millis(500));
                continue;
            }

            self.wine2linux_process = Some(child);
            return Ok(());
        }
    }

    fn cleanup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        if let Some(mut wine2linux_process) = self.wine2linux_process.take() {
            if let Err(error) = wine2linux_process.start_kill() {
                log::warn!("failed to stop wine2linux: {error}");
            }
        }

        Ok(())
    }

    fn wait_for_game_exit(&mut self, context: &AppContext) -> anyhow::Result<()> {
        while self.game_is_alive(context)? {
            thread::sleep(Duration::from_secs(1));
        }

        Ok(())
    }
}
