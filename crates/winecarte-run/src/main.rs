use anyhow::Context;
use clap::Parser;
use std::{env, path::PathBuf, process::Stdio};
use thiserror::Error;
use tokio::process;

mod handlers;
use handlers::get_handler;

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
