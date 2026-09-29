use crate::games::{self, Game};
use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    str,
};

pub const WINE2LINUX_EXE: &str = "wine2linux.exe";

pub struct Process {
    pub pid: u32,
    pub argv: Vec<String>,
}

impl Process {
    pub fn argv0(&self) -> &str {
        &self.argv[0]
    }
}

/// Every process under `proc_dir` with a readable, non-empty, UTF-8 command
/// line. Kernel threads and processes that vanish mid-scan are skipped.
pub fn processes(proc_dir: &Path) -> std::io::Result<impl Iterator<Item = Process>> {
    Ok(std::fs::read_dir(proc_dir)?.flatten().filter_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
        if cmdline.is_empty() {
            return None;
        }
        let argv = cmdline
            .split(|&b| b == 0)
            .map(|arg| str::from_utf8(arg).ok().map(str::to_owned))
            .collect::<Option<Vec<_>>>()?;
        // cmdline is NUL-terminated, so the split yields a trailing empty arg.
        let argv: Vec<String> = argv.into_iter().filter(|a| !a.is_empty()).collect();
        (!argv.is_empty()).then_some(Process { pid, argv })
    }))
}

/// Whether argv0, as a bare name or a Windows/Unix path, names `marker`.
pub fn exe_matches(argv0: &str, marker: &str) -> bool {
    argv0 == marker
        || argv0.ends_with(&format!("\\{marker}"))
        || argv0.ends_with(&format!("/{marker}"))
}

pub fn read_environ(pid: u32) -> Option<Vec<(OsString, OsString)>> {
    let data = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    Some(
        data.split(|&b| b == 0)
            .filter(|entry| !entry.is_empty())
            .filter_map(|entry| {
                let eq = entry.iter().position(|&b| b == b'=')?;
                Some((
                    OsStr::from_bytes(&entry[..eq]).to_owned(),
                    OsStr::from_bytes(&entry[eq + 1..]).to_owned(),
                ))
            })
            .collect(),
    )
}

pub fn env_value<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsStr> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_os_str())
}

/// The wine prefix a process runs in, from WINEPREFIX or, for Proton, the
/// compat data path. Normalised through path components so a trailing slash
/// does not make the two forms compare unequal.
pub fn wine_prefix_of(env: &[(OsString, OsString)]) -> Option<PathBuf> {
    let prefix = match env_value(env, "WINEPREFIX") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(env_value(env, "STEAM_COMPAT_DATA_PATH")?).join("pfx"),
    };
    Some(prefix.components().collect())
}

/// The first running process that belongs to a known game, skipping any argv0
/// the caller rejects (bridge stand-ins, wine2linux itself).
pub fn find_game_process(
    proc_dir: &Path,
    skip: impl Fn(&str) -> bool,
) -> std::io::Result<Option<(Process, &'static Game)>> {
    Ok(processes(proc_dir)?.find_map(|proc| {
        if skip(proc.argv0()) {
            return None;
        }
        let game = games::by_process(proc.argv0())?;
        Some((proc, game))
    }))
}

/// Whether any process's argv0 matches one of `markers`. wine2linux is never
/// counted, since it is named after no game but is launched from the game's
/// session and would otherwise keep a finished game looking alive.
pub fn game_is_alive(proc_dir: &Path, markers: &[&str]) -> std::io::Result<bool> {
    Ok(processes(proc_dir)?.any(|proc| {
        let argv0 = proc.argv0();
        markers.iter().any(|m| exe_matches(argv0, m)) && !exe_matches(argv0, WINE2LINUX_EXE)
    }))
}

/// Whether a wine2linux sender is already running against `prefix`, such as
/// one started by winecarte-run from the game's launch options.
pub fn sender_running_for_prefix(proc_dir: &Path, prefix: &Path) -> std::io::Result<bool> {
    Ok(processes(proc_dir)?.any(|proc| {
        exe_matches(proc.argv0(), WINE2LINUX_EXE)
            && proc.argv.iter().any(|a| a == "--from-wine")
            && read_environ(proc.pid)
                .as_deref()
                .and_then(wine_prefix_of)
                .is_some_and(|p| p == prefix)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn add_process(proc_dir: &Path, pid: u32, argv: &[&str]) {
        let pid_dir = proc_dir.join(pid.to_string());
        fs::create_dir_all(&pid_dir).unwrap();
        let cmdline: Vec<u8> = argv
            .iter()
            .flat_map(|s| s.bytes().chain(std::iter::once(0u8)))
            .collect();
        fs::write(pid_dir.join("cmdline"), &cmdline).unwrap();
    }

    fn add_kernel_thread(proc_dir: &Path, pid: u32) {
        let pid_dir = proc_dir.join(pid.to_string());
        fs::create_dir_all(&pid_dir).unwrap();
        fs::write(pid_dir.join("cmdline"), b"").unwrap();
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    #[test]
    fn detects_game_by_argv0() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 1234, &[r"Z:\games\Game.exe"]);
        assert!(game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn ignores_marker_in_args_not_argv0() {
        let dir = tempfile::tempdir().unwrap();
        add_process(
            dir.path(),
            1234,
            &[r"c:\windows\system32\launcher.exe", "/games/Game.exe"],
        );
        assert!(!game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn returns_false_with_no_matching_process() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 1234, &[r"Z:\games\OtherGame.exe"]);
        assert!(!game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn skips_kernel_threads() {
        let dir = tempfile::tempdir().unwrap();
        add_kernel_thread(dir.path(), 1);
        add_process(dir.path(), 1234, &[r"Z:\games\Game.exe"]);
        assert!(game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn skips_non_pid_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let non_pid = dir.path().join("net");
        fs::create_dir_all(&non_pid).unwrap();
        fs::write(non_pid.join("cmdline"), b"Game.exe\0").unwrap();
        assert!(!game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn excludes_wine2linux_from_results() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 1234, &[r"Z:\tools\wine2linux.exe"]);
        assert!(!game_is_alive(dir.path(), &["wine2linux.exe"]).unwrap());
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
        assert!(game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn only_launcher_remains_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        add_process(
            dir.path(),
            1234,
            &[r"c:\windows\system32\launcher.exe", "/games/Game.exe"],
        );
        assert!(!game_is_alive(dir.path(), &["Game.exe"]).unwrap());
    }

    #[test]
    fn find_game_process_respects_skip() {
        let dir = tempfile::tempdir().unwrap();
        add_process(dir.path(), 10, &[r"C:\winecarte\raceroom\RRRE.exe"]);
        add_process(dir.path(), 20, &[r"Z:\games\RRRE64.exe"]);
        let (proc, game) =
            find_game_process(dir.path(), |argv0| argv0.starts_with(r"C:\winecarte"))
                .unwrap()
                .unwrap();
        assert_eq!(proc.pid, 20);
        assert_eq!(game.name, "raceroom");
    }

    #[test]
    fn wine_prefix_normalises_trailing_slash() {
        let a = wine_prefix_of(&env(&[("WINEPREFIX", "/data/compat/1/pfx/")])).unwrap();
        let b = wine_prefix_of(&env(&[("STEAM_COMPAT_DATA_PATH", "/data/compat/1")])).unwrap();
        assert_eq!(a, b);
        assert!(wine_prefix_of(&env(&[("HOME", "/home/x")])).is_none());
    }

    #[test]
    fn game_table_is_consistent() {
        for game in games::GAMES {
            assert!(!game.process_names.is_empty(), "{}", game.name);
            // A sender needs a Steam session to run in, so games without an
            // appid are stand-ins only.
            if game.steam_appids.is_empty() {
                assert!(game.from_wine_args.is_empty(), "{}", game.name);
            } else {
                assert!(games::by_appid(game.steam_appids[0]).is_some_and(|g| g.name == game.name));
            }
        }
    }
}
