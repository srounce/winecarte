use anyhow::Context;
use clap::Parser;
use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    str,
    sync::atomic::{AtomicU32, Ordering},
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

static BRIDGE_COUNTER: AtomicU32 = AtomicU32::new(0);

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

    /// Path to wine2linux.exe. Falls back to $WINECARTE_WINE2LINUX_EXE then PATH.
    #[arg(long, env = "WINECARTE_WINE2LINUX_EXE")]
    wine2linux: Option<PathBuf>,

    /// Interval in milliseconds between game process scans.
    #[arg(long, default_value = "1000")]
    poll_ms: u64,
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
            log::info!("detected game process: {argv0} (pid {pid})");
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
    let count = BRIDGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let bridge_dir = env::temp_dir().join(format!("winehub-{}-{count}", std::process::id()));

    log::info!("creating bridge dir: {}", bridge_dir.display());
    std::fs::create_dir_all(&bridge_dir)
        .with_context(|| format!("failed to create bridge dir: {}", bridge_dir.display()))?;

    let bridge_exe = bridge_dir.join(&exe_name);
    log::info!(
        "copying {} → {}",
        wine2linux_exe.display(),
        bridge_exe.display()
    );
    std::fs::copy(wine2linux_exe, &bridge_exe)
        .with_context(|| format!("failed to copy wine2linux.exe to {}", bridge_exe.display()))?;

    let exe_path = wine_argv0_to_linux_path(argv0);
    let install_dir = exe_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_default();

    if !install_dir.as_os_str().is_empty() {
        log::info!("game install dir: {}", install_dir.display());
    } else {
        log::warn!("could not determine game install dir from argv0: {argv0}");
    }

    let context = GameContext {
        pid,
        exe_path: exe_path.unwrap_or_default(),
        install_dir,
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

    let bridge_exe_wine = linux_path_to_wine(&bridge_exe);
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

    log::info!(
        "removing bridge dir: {}",
        bridge.context.bridge_dir.display()
    );
    if let Err(e) = std::fs::remove_dir_all(&bridge.context.bridge_dir) {
        log::warn!("failed to remove bridge dir: {e}");
    }
}

fn extract_exe_name(argv0: &str) -> String {
    argv0
        .rsplit(|c: char| c == '\\' || c == '/')
        .next()
        .unwrap_or(argv0)
        .to_string()
}

fn linux_path_to_wine(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix('/') {
        format!("Z:\\{}", rest.replace('/', "\\"))
    } else {
        s.replace('/', "\\")
    }
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

    let path_env = env::var_os("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_env) {
        let candidate = dir.join("wine2linux.exe");
        if candidate.is_file() {
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join("wine2linux.exe");
            if candidate.is_file() {
                return Ok(candidate.canonicalize().unwrap_or(candidate));
            }
        }
    }

    anyhow::bail!(
        "could not find wine2linux.exe! Set WINECARTE_WINE2LINUX_EXE, --wine2linux, add to PATH, or place alongside winehub"
    )
}
