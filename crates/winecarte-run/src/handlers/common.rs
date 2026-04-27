use crate::{AppContext, find_on_path};
use anyhow::{Context, bail};
use std::{
    path::PathBuf,
    process::{Command as StdCommand, Stdio},
    str, thread,
    time::Duration,
};
use tokio::process;

pub(crate) fn resolve_runtime_launch_client(context: &AppContext) -> anyhow::Result<PathBuf> {
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

pub(crate) fn resolve_wine2linux_exe() -> anyhow::Result<PathBuf> {
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

pub(crate) fn launch_wine2linux(
    wine2linux_process: &mut Option<process::Child>,
    context: &AppContext,
    wine2linux_args: &[&str],
) -> anyhow::Result<()> {
    let runtime_launch_client = resolve_runtime_launch_client(context)?;
    let wine2linux_exe = resolve_wine2linux_exe()?;
    let bus_name = format!("com.steampowered.App{}", context.steam_appid);
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
            .args(wine2linux_args)
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

        *wine2linux_process = Some(child);
        return Ok(());
    }
}

pub(crate) fn cleanup_wine2linux(
    wine2linux_process: &mut Option<process::Child>,
) -> anyhow::Result<()> {
    if let Some(mut wine2linux_process) = wine2linux_process.take() {
        if let Err(error) = wine2linux_process.start_kill() {
            log::warn!("failed to stop wine2linux: {error}");
        }
    }

    Ok(())
}

pub(crate) fn game_is_alive(
    context: &AppContext,
    process_markers: &[&str],
    error_context: &str,
) -> anyhow::Result<bool> {
    let runtime_launch_client = resolve_runtime_launch_client(context)?;
    let bus_name = format!("com.steampowered.App{}", context.steam_appid);

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
        .with_context(|| error_context.to_string())?;

    if !output.status.success() {
        return Ok(false);
    }

    let stdout = str::from_utf8(&output.stdout).context("ps output was not valid UTF-8")?;

    Ok(stdout.lines().any(|line| {
        process_markers.iter().any(|marker| line.contains(marker))
            && !line.contains("wine2linux.exe")
    }))
}

pub(crate) fn wait_for_game_exit(
    context: &AppContext,
    process_markers: &[&str],
    error_context: &str,
) -> anyhow::Result<()> {
    while game_is_alive(context, process_markers, error_context)? {
        thread::sleep(Duration::from_secs(1));
    }

    Ok(())
}
