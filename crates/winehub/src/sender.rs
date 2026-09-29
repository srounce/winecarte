//! Starts the sending wine2linux inside the game's own wine session.
//!
//! Under Proton the game lives in a pressure-vessel container. The container
//! is a bubblewrap sandbox owned by our uid, and the kernel grants a user
//! namespace's owner full capabilities inside it, so an unprivileged process
//! can join the container's user and mount namespaces. From there the game's
//! wineserver socket under /tmp is visible, and a wine started with the game's
//! own environment attaches to the running session instead of creating one.
//! This is what `steam-runtime-launch-client` achieves through D-Bus, without
//! needing the game to have been launched with a launcher service.

use std::{
    ffi::OsString,
    fs::File,
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process;
use winecarte_core::{
    games::Game,
    launch::{self, SteamSession},
    procscan,
};

const PROC: &str = "/proc";

/// Returns the running sender, or None when one could not be started; the
/// receiver is still useful on its own, so this never fails the bridge.
pub fn launch(game: &Game, pid: u32, wine2linux_exe: &Path) -> Option<process::Child> {
    if game.from_wine_args.is_empty() {
        log::info!("{} has nothing to send; receiver only", game.name);
        return None;
    }

    let env = match procscan::read_environ(pid) {
        Some(env) => env,
        None => {
            log::warn!("cannot read environment of pid {pid}; not starting sender");
            return None;
        }
    };

    let prefix = match procscan::wine_prefix_of(&env) {
        Some(p) => p,
        None => {
            log::warn!(
                "pid {pid} has no WINEPREFIX or STEAM_COMPAT_DATA_PATH; not starting sender"
            );
            return None;
        }
    };
    log::info!("game wine prefix: {}", prefix.display());

    match procscan::sender_running_for_prefix(Path::new(PROC), &prefix) {
        Ok(true) => {
            log::info!(
                "a wine2linux sender is already running for this prefix (winecarte-run?); receiver only"
            );
            return None;
        }
        Ok(false) => {}
        Err(e) => log::warn!("failed to scan for an existing sender: {e}"),
    }

    match launch_in_namespaces(pid, &env, wine2linux_exe, game.from_wine_args) {
        Ok(child) => {
            log::info!("sender running inside the game's namespaces");
            return Some(child);
        }
        Err(e) => log::warn!("could not start sender in the game's namespaces: {e:#}"),
    }

    match launch_via_launcher_service(&env, wine2linux_exe, game.from_wine_args) {
        Ok(Some(child)) => {
            log::info!("sender running via steam-runtime-launch-client");
            Some(child)
        }
        Ok(None) => {
            log::warn!(
                "no launcher service available either; receiver only. \
                 Add STEAM_COMPAT_LAUNCHER_SERVICE=proton %command% to the game's launch options or use winecarte-run"
            );
            None
        }
        Err(e) => {
            log::warn!("could not start sender via launcher service: {e:#}; receiver only");
            None
        }
    }
}

fn launch_in_namespaces(
    pid: u32,
    env: &[(OsString, OsString)],
    wine2linux_exe: &Path,
    args: &[&str],
) -> anyhow::Result<process::Child> {
    let joins = namespaces_to_join(pid)?;
    if joins.is_empty() {
        log::info!("pid {pid} shares our namespaces; launching sender directly");
    }

    // WINELOADER is what the game itself was started through, so it is the
    // right wine for its prefix. Proton exports it; plain wine may not.
    let program = procscan::env_value(env, "WINELOADER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("wine"));

    let mut command = process::Command::new(&program);
    command
        .arg(wine2linux_exe)
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Set by wine's preloader for the process it execs, and inherited by the
    // game. Left in place it would make our loader skip the preloader.
    command.env_remove("WINELOADERNOEXEC");

    // Runs in the forked child before exec. Joining a user namespace is only
    // allowed for a single-threaded process, which the child is at this point
    // and the tokio parent never is. The mount namespace join resets cwd and
    // root to the container's, which is where wine should run anyway.
    unsafe {
        command.pre_exec(move || {
            for (file, flag) in &joins {
                if libc::setns(file.as_raw_fd(), *flag) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    log::info!(
        "launching sender: {} {} {:?}",
        program.display(),
        wine2linux_exe.display(),
        args
    );
    Ok(command.spawn()?)
}

/// The namespaces of `pid` that differ from ours, as open fds ready for
/// setns. Joining a namespace we are already in is rejected by the kernel, so
/// identical ones are left out. Order matters: the user namespace must come
/// first because it is what grants the capability to join the mount one.
fn namespaces_to_join(pid: u32) -> io::Result<Vec<(File, libc::c_int)>> {
    let mut joins = Vec::new();
    for (name, flag) in [("user", libc::CLONE_NEWUSER), ("mnt", libc::CLONE_NEWNS)] {
        let ours = std::fs::read_link(format!("{PROC}/self/ns/{name}"))?;
        let theirs = std::fs::read_link(format!("{PROC}/{pid}/ns/{name}"))?;
        if ours != theirs {
            joins.push((File::open(format!("{PROC}/{pid}/ns/{name}"))?, flag));
        }
    }

    // Not joined: setns on a pid namespace only affects later children, and
    // wine talks to wineserver over a socket rather than by pid. Worth knowing
    // about if it ever happens, though.
    let ours = std::fs::read_link(format!("{PROC}/self/ns/pid"))?;
    let theirs = std::fs::read_link(format!("{PROC}/{pid}/ns/pid"))?;
    if ours != theirs {
        log::warn!("pid {pid} runs in a separate pid namespace; sender will stay in ours");
    }

    Ok(joins)
}

/// The `--bus-name` route, usable only when the game was started with a
/// launcher service. Returns Ok(None) when the environment shows it was not.
fn launch_via_launcher_service(
    env: &[(OsString, OsString)],
    wine2linux_exe: &Path,
    args: &[&str],
) -> anyhow::Result<Option<process::Child>> {
    if procscan::env_value(env, "STEAM_COMPAT_LAUNCHER_SERVICE").is_none() {
        return Ok(None);
    }
    let (Some(compat_data_path), Some(tool_paths), Some(steam_appid)) = (
        procscan::env_value(env, "STEAM_COMPAT_DATA_PATH"),
        procscan::env_value(env, "STEAM_COMPAT_TOOL_PATHS").and_then(|v| v.to_str()),
        procscan::env_value(env, "SteamAppId").and_then(|v| v.to_str()),
    ) else {
        return Ok(None);
    };
    let Some((_, steam_linux_runtime_path)) = SteamSession::split_tool_paths(tool_paths) else {
        return Ok(None);
    };

    let compat_data_path = PathBuf::from(compat_data_path).join("pfx");
    let session = SteamSession {
        compat_data_path: &compat_data_path,
        steam_linux_runtime_path: &steam_linux_runtime_path,
        steam_appid,
    };
    launch::launch_via_launcher_service(&session, wine2linux_exe, args).map(Some)
}
