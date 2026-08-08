use anyhow::Context;
use clap::Parser;
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    str,
};
use tokio::{
    io::AsyncBufReadExt,
    process,
    signal::unix::{SignalKind, signal},
    time,
};
use tokio_util::sync::CancellationToken;

mod games;
use games::{GAMES, GameBridge, GameContext};

/// Bridge directories live inside the prefix so the fake game exe gets a real
/// `C:\` path, and so tools that inspect it are looking at a stable location
/// that outlives any single session.
const BRIDGE_DIR_NAME: &str = "winecarte";

#[derive(Parser, Debug)]
#[command(version, about = "SimHub shared memory bridge for Wine games")]
struct Args {
    /// Wine prefix containing the SimHub installation.
    /// Defaults to $WINEPREFIX if not specified.
    #[arg(long, env = "WINEPREFIX")]
    prefix: Option<PathBuf>,

    /// Path to the Wine binary to use when launching the bridge.
    #[arg(long, env = "WINEHUB_WINE", default_value = "wine")]
    wine: String,

    /// Path to wine2linux.exe. Falls back to $WINECARTE_WINE2LINUX_EXE, then a
    /// copy alongside winehub.exe, then PATH.
    #[arg(long, env = "WINECARTE_WINE2LINUX_EXE")]
    wine2linux: Option<PathBuf>,

    /// Interval in milliseconds between game process scans.
    #[arg(long, default_value = "1000")]
    poll_ms: u64,

    /// Remove all persistent bridge directories from the prefix on startup.
    #[arg(long)]
    clean_bridges: bool,
}

struct Bridge {
    game: &'static GameBridge,
    context: GameContext,
    process: process::Child,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_env("WINECARTE_LOG_LEVEL")
        .format_level(true)
        .format_module_path(false)
        .format_target(true)
        .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Seconds))
        .try_init()?;

    let args = Args::parse();

    let prefix = args
        .prefix
        .ok_or_else(|| anyhow::anyhow!("no Wine prefix specified; set --prefix or WINEPREFIX"))?;

    if !prefix.exists() {
        anyhow::bail!("Wine prefix does not exist: {}", prefix.display());
    }

    if args.clean_bridges {
        let root = bridge_root(&prefix);
        log::info!("removing bridge root: {}", root.display());
        match std::fs::remove_dir_all(&root) {
            Ok(()) => log::info!("bridge root removed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::info!("no bridge root to remove")
            }
            Err(e) => anyhow::bail!("failed to remove bridge root {}: {e}", root.display()),
        }
    }

    let wine2linux_exe = resolve_wine2linux_exe(args.wine2linux)?;
    log::info!("using wine2linux: {}", wine2linux_exe.display());
    log::info!("using wine prefix: {}", prefix.display());

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

    log::info!("scanning for game processes every {}ms", args.poll_ms);
    let poll_interval = std::time::Duration::from_millis(args.poll_ms);
    let mut active: Option<Bridge> = None;

    loop {
        if shutdown.is_cancelled() {
            if let Some(bridge) = active.take() {
                cleanup_bridge(bridge).await;
            }
            break;
        }

        if active.is_some() {
            let pid = active.as_ref().unwrap().context.pid;
            if !is_pid_alive(pid) {
                log::info!("game process {pid} exited, stopping bridge");
                cleanup_bridge(active.take().unwrap()).await;
            }
        } else if let Some((pid, argv0, game)) = find_game_process() {
            log::info!("detected {} process: {argv0} (pid {pid})", game.name);
            match start_bridge(game, pid, &argv0, &prefix, &args.wine, &wine2linux_exe).await {
                Ok(bridge) => {
                    log::info!("bridge running for {argv0}");
                    active = Some(bridge);
                }
                Err(e) => log::error!("failed to start bridge: {e:#}"),
            }
        }

        tokio::select! {
            _ = time::sleep(poll_interval) => {},
            _ = shutdown.cancelled() => {},
        }
    }

    Ok(())
}

fn find_game_process() -> Option<(u32, String, &'static GameBridge)> {
    let proc_dir = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(e) => {
            log::warn!("failed to read /proc: {e}");
            return None;
        }
    };

    for entry in proc_dir.flatten() {
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

        if is_bridge_exe(argv0) {
            continue;
        }

        for game in GAMES {
            if game.process_names.iter().any(|&m| exe_matches(argv0, m)) {
                let pid: u32 = match entry.file_name().to_string_lossy().parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                return Some((pid, argv0.to_string(), game));
            }
        }
    }

    None
}

fn exe_matches(argv0: &str, marker: &str) -> bool {
    argv0 == marker
        || argv0.ends_with(&format!("\\{marker}"))
        || argv0.ends_with(&format!("/{marker}"))
}

fn is_pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

async fn start_bridge(
    game: &'static GameBridge,
    pid: u32,
    argv0: &str,
    prefix: &Path,
    wine: &str,
    wine2linux_exe: &Path,
) -> anyhow::Result<Bridge> {
    let exe_name = extract_exe_name(argv0);
    let bridge_dir = bridge_root(prefix).join(game.name);

    let exe_path = wine_argv0_to_linux_path(argv0);
    let install_dir = exe_path
        .as_deref()
        .and_then(Path::parent)
        .map(PathBuf::from);

    match &install_dir {
        Some(dir) => log::info!("game install dir: {}", dir.display()),
        None => log::warn!("could not determine game install dir from argv0: {argv0}"),
    }

    reset_bridge_dir(&bridge_dir)?;

    if game.link_sibling_dirs {
        match &install_dir {
            Some(dir) => link_sibling_dirs(dir, &bridge_dir, &exe_name),
            None => log::warn!("cannot link sibling dirs without an install dir"),
        }
    }

    let bridge_exe = bridge_dir.join(&exe_name);
    // Nothing named after the exe is ever linked, so this can only fire on a
    // bug. Copying onto a symlink would write into the real install.
    if bridge_exe.symlink_metadata().is_ok() {
        anyhow::bail!(
            "refusing to overwrite existing bridge exe: {}",
            bridge_exe.display()
        );
    }

    log::info!(
        "copying {} → {}",
        wine2linux_exe.display(),
        bridge_exe.display()
    );
    std::fs::copy(wine2linux_exe, &bridge_exe)
        .with_context(|| format!("failed to copy wine2linux.exe to {}", bridge_exe.display()))?;

    let context = GameContext {
        pid,
        exe_path: exe_path.unwrap_or_default(),
        install_dir: install_dir.unwrap_or_default(),
        bridge_dir: bridge_dir.clone(),
        compat_data_path: read_compat_data_path(pid),
    };

    if let Some(cdp) = &context.compat_data_path {
        log::info!("game compat data path: {}", cdp.display());
    }

    if let Some(setup) = game.setup {
        log::info!("running game setup hook");
        if let Err(e) = setup(&context) {
            let _ = std::fs::remove_dir_all(&bridge_dir);
            return Err(e.context("game setup hook failed"));
        }
    }

    let bridge_exe_wine = bridge_exe_dos_path(game.name, &exe_name);
    let mut command = process::Command::new(wine);
    command
        .arg(&bridge_exe_wine)
        .args(game.from_linux_args)
        .env("WINEPREFIX", prefix)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(shm_dir) = flatpak_dev_shm_dir() {
        log::info!("using flatpak shm dir: {}", shm_dir.display());
        command.args(["--dest-root".as_ref(), shm_dir.as_os_str()]);
    }

    log::info!("launching: {wine} {bridge_exe_wine}");
    log::info!("bridge args: {:?}", game.from_linux_args);
    let mut child = command
        .spawn()
        .context("failed to spawn wine bridge process")?;

    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(forward_wine_output(stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(forward_wine_output(stderr));
    }

    Ok(Bridge {
        game,
        context,
        process: child,
    })
}

fn bridge_root(prefix: &Path) -> PathBuf {
    prefix.join("drive_c").join(BRIDGE_DIR_NAME)
}

/// The DOS path a bridge exe is launched from, which is also what wine reports
/// as that process's argv0. Built from parts rather than translated from the
/// Linux path, so a symlinked or non-canonical WINEPREFIX cannot throw it off.
/// `is_bridge_exe` has to recognise whatever this produces.
fn bridge_exe_dos_path(game_name: &str, exe_name: &str) -> String {
    format!(r"C:\{BRIDGE_DIR_NAME}\{game_name}\{exe_name}")
}

/// A bridge exe is named after the game it stands in for, so it matches the
/// same process names the scan looks for. Without this, a second winehub — or
/// a restart while an earlier bridge is still alive — bridges the bridge.
/// Matching on the DOS path catches bridges from any prefix, not just ours.
fn is_bridge_exe(argv0: &str) -> bool {
    argv0
        .to_ascii_lowercase()
        .starts_with(&format!(r"c:\{BRIDGE_DIR_NAME}\"))
}

/// Rebuild from scratch rather than reusing: unlinking leaves a previous
/// bridge's exe intact for as long as it is still mapped.
fn reset_bridge_dir(bridge_dir: &Path) -> anyhow::Result<()> {
    // symlink_metadata, so a link squatting on the path is unlinked rather than
    // followed into whatever it points at.
    match bridge_dir.symlink_metadata() {
        Ok(meta) if meta.is_dir() => {
            log::info!("removing stale bridge dir: {}", bridge_dir.display());
            std::fs::remove_dir_all(bridge_dir)
                .with_context(|| format!("failed to remove bridge dir: {}", bridge_dir.display()))?
        }
        Ok(_) => {
            log::warn!("removing non-directory at {}", bridge_dir.display());
            std::fs::remove_file(bridge_dir)
                .with_context(|| format!("failed to remove {}", bridge_dir.display()))?
        }
        Err(_) => {}
    }

    log::info!("creating bridge dir: {}", bridge_dir.display());
    std::fs::create_dir_all(bridge_dir)
        .with_context(|| format!("failed to create bridge dir: {}", bridge_dir.display()))
}

/// Symlinks every directory sitting alongside the game exe into the bridge dir,
/// so tools that resolve game data relative to the running exe find it.
///
/// Best effort throughout: shared memory bridging is the primary job and does
/// not depend on these links, so nothing here is worth failing a bridge over.
fn link_sibling_dirs(install_dir: &Path, bridge_dir: &Path, exe_name: &str) {
    let entries = match std::fs::read_dir(install_dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("failed to read install dir {}: {e}", install_dir.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        // The bridge exe is copied into this same directory, and copying onto a
        // symlink would write through it into the real install.
        if name == *exe_name {
            continue;
        }

        // is_dir follows links, so a directory reached through one still counts
        // and a broken link is skipped.
        let target = entry.path();
        if !target.is_dir() {
            continue;
        }

        let link = bridge_dir.join(&name);
        match std::os::unix::fs::symlink(&target, &link) {
            Ok(()) => log::info!("linked {} → {}", target.display(), link.display()),
            Err(e) => log::warn!("failed to symlink {}: {e}", link.display()),
        }
    }
}

fn flatpak_dev_shm_dir() -> Option<PathBuf> {
    let path = PathBuf::from(env::var_os("XDG_RUNTIME_DIR")?)
        .join(".flatpak/com.valvesoftware.Steam/dev-shm");
    path.exists().then_some(path)
}

async fn forward_wine_output<R>(reader: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = tokio::io::BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log::info!(target: "wine", "{line}");
    }
}

async fn cleanup_bridge(mut bridge: Bridge) {
    log::info!("stopping bridge for pid {}", bridge.context.pid);
    if let Err(e) = bridge.process.start_kill() {
        log::warn!("failed to kill bridge process: {e}");
    } else {
        let _ = bridge.process.wait().await;
    }

    if let Some(teardown) = bridge.game.teardown {
        log::info!("running game teardown hook");
        if let Err(e) = teardown(&bridge.context) {
            log::warn!("teardown hook failed: {e:#}");
        }
    }

    // The bridge dir deliberately outlives the session: tools that act on game
    // exit still resolve paths through it after winehub has moved on.
}

fn extract_exe_name(argv0: &str) -> String {
    argv0
        .rsplit(|c: char| c == '\\' || c == '/')
        .next()
        .unwrap_or(argv0)
        .to_string()
}

fn wine_argv0_to_linux_path(argv0: &str) -> Option<PathBuf> {
    let argv0 = argv0.trim_matches('"');
    if let Some(rest) = argv0
        .strip_prefix("Z:\\")
        .or_else(|| argv0.strip_prefix("z:\\"))
    {
        Some(PathBuf::from("/").join(rest.replace('\\', "/")))
    } else if argv0.starts_with('/') {
        Some(PathBuf::from(argv0))
    } else {
        None
    }
}

fn read_compat_data_path(pid: u32) -> Option<PathBuf> {
    let data = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    data.split(|&b| b == 0).find_map(|entry| {
        let s = str::from_utf8(entry).ok()?;
        s.strip_prefix("STEAM_COMPAT_DATA_PATH=").map(PathBuf::from)
    })
}

fn resolve_wine2linux_exe(override_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(path) = override_path {
        if path.exists() {
            return Ok(path.canonicalize().unwrap_or(path));
        }
        anyhow::bail!("wine2linux path does not exist: {}", path.display());
    }

    // Prefer a sibling build so a winehub copy stays paired with its own
    // wine2linux rather than whichever one happens to be installed on PATH.
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join("wine2linux.exe");
            if candidate.is_file() {
                return Ok(candidate.canonicalize().unwrap_or(candidate));
            }
        }
    }

    let path_env = env::var_os("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_env) {
        let candidate = dir.join("wine2linux.exe");
        if candidate.is_file() {
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    anyhow::bail!(
        "could not find wine2linux.exe! Set WINECARTE_WINE2LINUX_EXE, --wine2linux, add to PATH, or place alongside winehub"
    )
}
