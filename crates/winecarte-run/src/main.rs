use anyhow::Context;
use clap::Parser;
use std::{path::{Path, PathBuf}, process::Stdio};
use thiserror::Error;
use tokio::process;
use env_logger;

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

    #[error("No STEAM_COMPAT_DATA_PATH provided")]
    MissingCompatDataPath,
    #[error("Invalid STEAM_COMPAT_DATA_PATH provided")]
    InvalidCompatDataPath,

    #[error("No STEAM_COMPAT_TOOL_PATHS provided")]
    MissingCompatToolPath,
    #[error("Invalid STEAM_COMPAT_TOOL_PATHS provided")]
    InvalidCompatToolPath,
}

fn get_handler(appid: String) -> Box<dyn AppHandler> {
    match appid.as_str() {
        "244210" => Box::new(AssettoCorsaHandler::default()),
        // "2399420" => Box::new(LeMansUltimateHandler::default()),
        _ => Box::new(EmptyHandler::default()),
    }
}

fn print_env() {
    let mut vars = std::env::vars().collect::<Vec<_>>();
    vars.sort_by_key(|(key, _)| key.to_lowercase());
    for (k, v) in vars {
        println!("env: {k} => {v}");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // print_env();

    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_env("WINECARTE_LOG_LEVEL")
        .format_level(true)
        .format_module_path(true)
        .format_target(true)
        .try_init()?;

    let args = Args::parse();

    let appid = std::env::var("SteamAppId")
        .map_err(|_| StartupError::MissingAppId)?;

    let compat_data_path = std::env::var("STEAM_COMPAT_DATA_PATH")
        .map_err(|_| StartupError::MissingCompatDataPath)
        .map(PathBuf::from)
        .map(|mut path| {
            path.push("pfx");
            path
        })
        .and_then(|path| {
            match path.exists() {
                true => Ok(path),
                false => Err(StartupError::InvalidCompatDataPath),
            }
        })?;

    let compat_tool_path = std::env::var("STEAM_COMPAT_TOOL_PATHS")
        .map_err(|_| StartupError::MissingCompatToolPath)
        .and_then(|value| {
            value
                .split_once(":")
                .map(|(first, _)| first.to_string())
                .ok_or(StartupError::InvalidCompatToolPath)
        })
        .map(PathBuf::from)
        .and_then(|path| {
            match path.exists() {
                true => Ok(path),
                false => Err(StartupError::InvalidCompatDataPath),
            }
        })?;

    log::info!("Wrapping AppId: {appid}");
    log::info!("Proton path: {}", compat_tool_path.to_str().unwrap_or_default());
    log::info!("Prefix path: {}", compat_data_path.to_str().unwrap_or_default());

    let handler = get_handler(appid);

    handler.setup();

    if args.startup_command.is_empty() {
        return Err(StartupError::MissingStartupCommand.into());
    }

    let (startup_command, startup_args) = { args.startup_command.split_at(1) };

    let mut command = process::Command::new(startup_command.first().unwrap());
    command
        .args(startup_args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    println!("Running: {command:?}");
    let mut child_process = command.spawn().with_context(|| "Child command failure")?;
    handler.on_start();

    let status = child_process.wait().await?;

    println!("Exited with status {:?}", status.code().unwrap());

    handler.cleanup();

    Ok(())
}

trait AppHandler {
    fn setup(&self) {}
    fn on_start(&self) {}
    fn cleanup(&self) {}
}

#[derive(Default)]
struct EmptyHandler {}

impl AppHandler for EmptyHandler {
    fn setup(&self) {
        println!("EmptyHandler: Running setup");
    }

    fn on_start(&self) {
        println!("EmptyHandler: Running on_start");
    }

    fn cleanup(&self) {
        println!("EmptyHandler: Running cleanup");
    }
}

#[derive(Default)]
struct AssettoCorsaHandler {}

impl AppHandler for AssettoCorsaHandler {}
