use anyhow::Context;
use clap::Parser;
use std::process::Stdio;
use thiserror::Error;
use tokio::process;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(short = 'i', long)]
    appid: Option<usize>,

    #[arg(last(false))]
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
}

fn get_handler(appid: usize) -> Box<dyn AppHandler> {
    match appid {
        244210 => Box::new(AssettoCorsaHandler::default()),
        _ => Box::new(EmptyHandler::default()),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    //std::env::vars().into_iter().for_each(|v| println!("env: {v:?}"));

    let args = Args::parse();

    let appid = args
        .appid
        .ok_or(|| StartupError::MissingAppId)
        .or_else(|_| {
            std::env::var("SteamAppId")
                .map_err(|_| StartupError::MissingAppId)
                .and_then(|app_id| {
                    app_id
                        .parse::<usize>()
                        .map_err(|_| StartupError::InvalidAppId)
                })
        })?;

    println!("Wrapping AppId {appid}");
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
    let mut handle = command.spawn().with_context(|| "Child command failure")?;
    handler.on_start();

    let status = handle.wait().await?;

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
