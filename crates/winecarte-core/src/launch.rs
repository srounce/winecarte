//! Launching wine2linux inside a game's Proton container through Steam's
//! command launcher service, and locating the binaries involved.

use anyhow::{Context, bail};
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};
use tokio::process;

/// Where a game's Steam Linux Runtime session can be reached.
pub struct SteamSession<'a> {
    /// The `pfx` directory under STEAM_COMPAT_DATA_PATH.
    pub compat_data_path: &'a Path,
    pub steam_linux_runtime_path: &'a Path,
    pub steam_appid: &'a str,
}

impl<'a> SteamSession<'a> {
    /// Splits STEAM_COMPAT_TOOL_PATHS into the compat tool and runtime paths.
    pub fn split_tool_paths(value: &str) -> Option<(PathBuf, PathBuf)> {
        let (tool, runtime) = value.split_once(':')?;
        Some((PathBuf::from(tool), PathBuf::from(runtime)))
    }
}

pub fn find_on_path(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

pub fn resolve_runtime_launch_client(steam_linux_runtime_path: &Path) -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os("WINECARTE_RUNTIME_LAUNCH_CLIENT") {
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
        steam_linux_runtime_path.join("pressure-vessel/bin/steam-runtime-launch-client"),
        steam_linux_runtime_path.join("ubuntu12_64/steam-runtime-launch-client"),
    ];
    if let Some(found) = candidates.into_iter().find(|c| c.exists()) {
        return Ok(found);
    }

    bail!("could not find steam-runtime-launch-client; set WINECARTE_RUNTIME_LAUNCH_CLIENT")
}

/// Locates wine2linux.exe: an explicit path, then a sibling of the running
/// binary so a copy stays paired with its own build, then PATH.
pub fn resolve_wine2linux_exe(override_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(path) = override_path {
        if path.exists() {
            return Ok(path.canonicalize().unwrap_or(path));
        }
        bail!("wine2linux path does not exist: {}", path.display());
    }

    if let Some(candidate) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("wine2linux.exe")))
        .filter(|c| c.is_file())
    {
        return Ok(candidate.canonicalize().unwrap_or(candidate));
    }

    if let Some(path) = find_on_path("wine2linux.exe") {
        return Ok(path.canonicalize().unwrap_or(path));
    }

    bail!(
        "could not find wine2linux.exe! Set WINECARTE_WINE2LINUX_EXE, add it to PATH, or place it alongside this binary"
    )
}

/// Runs wine2linux inside the game's container via
/// `steam-runtime-launch-client`. Requires the game to have been launched with
/// STEAM_COMPAT_LAUNCHER_SERVICE=proton; retries briefly because the service
/// comes up slightly after the game process appears.
pub fn launch_via_launcher_service(
    session: &SteamSession,
    wine2linux_exe: &Path,
    wine2linux_args: &[&str],
) -> anyhow::Result<process::Child> {
    let runtime_launch_client = resolve_runtime_launch_client(session.steam_linux_runtime_path)?;
    let bus_name = format!("com.steampowered.App{}", session.steam_appid);
    let retry_deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let mut command = process::Command::new(&runtime_launch_client);
        command
            .arg("--bus-name")
            .arg(&bus_name)
            .arg("--directory=")
            .arg("--")
            .arg("wine")
            .arg(wine2linux_exe)
            .args(wine2linux_args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .env("STEAM_COMPAT_DATA_PATH", session.compat_data_path);

        log::info!("launching wine2linux via steam-runtime-launch-client: {command:?}");
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to launch {}", wine2linux_exe.display()))?;

        thread::sleep(Duration::from_millis(250));
        let Some(status) = child
            .try_wait()
            .context("failed to query wine2linux launcher status")?
        else {
            return Ok(child);
        };

        if Instant::now() >= retry_deadline {
            bail!(
                "wine2linux launcher exited before the Steam command-launcher service became available; last status {:?}. \
Make sure Steam launch options include STEAM_COMPAT_LAUNCHER_SERVICE=proton",
                status.code()
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Kills wine2linux and waits for it to exit, so wineserver releases the Win32
/// handles it held on the destination files. Without the wait a quick game
/// restart hits sharing violations when the next wine2linux opens them.
pub async fn stop_wine2linux(child: &mut process::Child) {
    if let Err(error) = child.start_kill() {
        log::warn!("failed to stop wine2linux: {error}");
    } else {
        let _ = child.wait().await;
    }
}
