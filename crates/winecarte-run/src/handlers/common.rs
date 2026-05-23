use crate::{AppContext, find_on_path};
use anyhow::{Context, bail};
use std::{path::PathBuf, process::Stdio, str, thread, time::Duration};
use tokio::process;

pub(crate) enum RunnerState {
    /// The game command yet has not been launched yet.
    PreStart,
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
    if let Some(path) = std::env::var_os("WINECARTE_WINE2LINUX_EXE") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path.canonicalize().unwrap_or(path));
        }

        bail!(
            "WINECARTE_WINE2LINUX_EXE points to a missing path: {}",
            path.display()
        );
    }

    if let Some(path) = find_on_path("wine2linux.exe") {
        return Ok(path.canonicalize().unwrap_or(path));
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join("wine2linux.exe");
            if candidate.is_file() {
                return Ok(candidate.canonicalize().unwrap_or(candidate));
            }
        }
    }

    bail!(
        "could not find wine2linux.exe! Set WINECARTE_WINE2LINUX_EXE, --wine2linux, add to PATH, or place alongside winehub"
    )
}

pub(crate) fn launch_wine2linux(
    wine2linux_process: &mut Option<process::Child>,
    context: &AppContext,
    wine2linux_args: &[&str],
) -> anyhow::Result<()> {
    let runtime_launch_client = resolve_runtime_launch_client(context)?;
    let wine2linux_exe = resolve_wine2linux_exe()?;
    log::info!("Using wine2linux: {}", wine2linux_exe.display());
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

        log::info!("Launching wine2linux helper: {command:?}");
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

pub(crate) async fn cleanup_wine2linux(
    wine2linux_process: &mut Option<process::Child>,
) -> anyhow::Result<()> {
    if let Some(mut process) = wine2linux_process.take() {
        if let Err(error) = process.start_kill() {
            log::warn!("failed to stop wine2linux: {error}");
        } else {
            // Wait for the process to fully exit so Wine's wineserver releases
            // the Win32 file handles wine2linux held on the destination files.
            // Without this, a quick game restart can hit sharing violations when
            // the new wine2linux tries to open those files.
            let _ = process.wait().await;
        }
    }

    Ok(())
}

pub(crate) fn game_is_alive(process_markers: &[&str]) -> anyhow::Result<bool> {
    proc_game_is_alive_in(std::path::Path::new("/proc"), process_markers)
}

fn exe_matches(argv0: &str, marker: &str) -> bool {
    argv0 == marker
        || argv0.ends_with(&format!("\\{marker}"))
        || argv0.ends_with(&format!("/{marker}"))
}

fn proc_game_is_alive_in(
    proc_dir: &std::path::Path,
    process_markers: &[&str],
) -> anyhow::Result<bool> {
    for entry in std::fs::read_dir(proc_dir).context("failed to read /proc")? {
        let entry = entry.context("failed to read /proc entry")?;

        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }

        let cmdline = match std::fs::read(entry.path().join("cmdline")) {
            Ok(data) => data,
            Err(_) => continue,
        };

        if cmdline.is_empty() {
            continue;
        }

        let argv0_bytes = cmdline.split(|&b| b == 0).next().unwrap_or(&[]);
        let argv0 = match str::from_utf8(argv0_bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if process_markers
            .iter()
            .any(|marker| exe_matches(argv0, marker))
            && !exe_matches(argv0, "wine2linux.exe")
        {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn add_process(proc_dir: &std::path::Path, pid: u32, argv: &[&str]) {
        let pid_dir = proc_dir.join(pid.to_string());
        fs::create_dir_all(&pid_dir).unwrap();
        let cmdline: Vec<u8> = argv
            .iter()
            .flat_map(|s| s.bytes().chain(std::iter::once(0u8)))
            .collect();
        fs::write(pid_dir.join("cmdline"), &cmdline).unwrap();
    }

    fn add_kernel_thread(proc_dir: &std::path::Path, pid: u32) {
        let pid_dir = proc_dir.join(pid.to_string());
        fs::create_dir_all(&pid_dir).unwrap();
        fs::write(pid_dir.join("cmdline"), b"").unwrap();
    }

    #[test]
    fn detects_game_by_argv0() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 1234, &[r"Z:\games\Game.exe"]);
        assert!(proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn ignores_marker_in_args_not_argv0() {
        let dir = tempfile::tempdir().unwrap();
        add_process(
            dir.path(),
            1234,
            &[r"c:\windows\system32\launcher.exe", "/games/Game.exe"],
        );
        assert!(!proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn returns_false_with_no_matching_process() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 1234, &[r"Z:\games\OtherGame.exe"]);
        assert!(!proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn skips_kernel_threads() {
        let dir = tempfile::tempdir().unwrap();
        add_kernel_thread(dir.path(), 1);
        add_process(dir.path(), 1234, &[r"Z:\games\Game.exe"]);
        assert!(proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn skips_non_pid_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let non_pid = dir.path().join("net");
        fs::create_dir_all(&non_pid).unwrap();
        fs::write(non_pid.join("cmdline"), b"Game.exe\0").unwrap();
        assert!(!proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn excludes_wine2linux_from_results() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 1234, &[r"Z:\tools\wine2linux.exe"]);
        assert!(!proc_game_is_alive_in(dir.path(), &["wine2linux.exe"]).unwrap());
    }

    #[test]
    fn game_and_launcher_coexist_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        add_process(
            dir.path(),
            1234,
            &[r"c:\windows\system32\launcher.exe", "/games/Game.exe"],
        );
        add_process(dir.path(), 1235, &[r"Z:\games\Game.exe"]);
        assert!(proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn only_launcher_remains_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        add_process(
            dir.path(),
            1234,
            &[r"c:\windows\system32\launcher.exe", "/games/Game.exe"],
        );
        assert!(!proc_game_is_alive_in(dir.path(), &["Game.exe"]).unwrap());
    }
}
