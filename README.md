# Winecarte

## Quickstart

Winecarte lets dashboards, overlays and other telemetry apps see data from sim racing games running through Proton on Linux. Check your game is in the [supported games](#supported-games) list, then:

1. Download the [latest release](../../releases/latest) and extract it.
2. Open a terminal in the extracted folder and run:

   ```
   mkdir -p ~/.local/bin
   cp winecarte-run winehub wine2linux.exe ~/.local/bin/
   ```

3. Follow whichever of the two setups below matches the apps you use.

### For Linux apps

In Steam, right-click the game, choose Properties, and paste this into Launch Options:

```
winecarte-run %command%
```

Start the game as usual. Repeat for each game you want to use.

### For Windows apps running in their own Wine prefix

Open a terminal and run this, replacing the path with the Wine prefix your app is installed in:

```
winehub --prefix /path/to/wine-prefix
```

Leave it running and start your games as usual. `winecarte-run` is not needed with this setup, and the games need no additional Steam launch options.

## About

Racing simulators publish live telemetry (physics, graphics state, lap timing, etc.) via Win32 named shared memory. When these games run under Proton on Linux, that shared memory is confined to the Wine environment and unreachable by native Linux applications such as dashboards, overlays, and telemetry recorders.

Winecarte bridges that gap by mirroring Win32 named mappings into Linux shared memory files under `/dev/shm`, making them available to any Linux process as though the game were running natively.

It covers two use cases:

- [Exposing shared memory to Linux](#exposing-shared-memory-to-linux), for native Linux applications.
- [Bridging into another Wine prefix](#bridging-into-another-wine-prefix), for Windows applications that run in a prefix separate from the games'.

## How it works

**`wine2linux.exe`** is a Windows executable that runs under Wine and does the copying, in either direction: Win32 named mappings into Linux files (`--from-wine`), or Linux files into Win32 named mappings (`--from-linux`). The other two binaries exist to start it in the right place at the right time.

**`winecarte-run`** is a native Linux binary used as a Steam launch wrapper. It spawns the real launcher, watches `/proc` until the game process appears, then uses `steam-runtime-launch-client` to launch `wine2linux.exe` inside the running Proton instance. When the game exits, it tears `wine2linux.exe` down cleanly.

```
Steam
 └─ winecarte-run (Linux)
     ├─ spawns game launcher
     ├─ waits for game process in /proc
     └─ launches wine2linux.exe (via steam-runtime-launch-client)
         ├─ reads Win32 named mappings created by the game
         └─ writes to /dev/shm/<mapping-name>
                              └─ read by dashboards, overlays, etc.
```

**`winehub`** is a native Linux daemon. It polls `/proc` for any supported game and, when one appears, starts two `wine2linux.exe` instances, stopping both when the game exits:

```
winehub (Linux)
 ├─ polls /proc for a supported game
 ├─ sender: wine2linux.exe in the game's Wine session
 │   └─ Win32 mappings → /dev/shm/<mapping-name>
 └─ receiver: wine2linux.exe in --prefix
     └─ /dev/shm/<mapping-name> → Win32 mappings
                                   └─ read by Windows apps in that prefix
```

## Supported games

| Game | Steam App ID | Shared memory files |
|---|---|---|
| Assetto Corsa | 244210 | `acpmf_physics`, `acpmf_graphics`, `acpmf_static`, `acpmf_simhub_v2`, `acpmf_crewchief`, `acpmf_secondMonitor` |
| Assetto Corsa Competizione | 805550 | `acpmf_physics`, `acpmf_graphics`, `acpmf_static` |
| Assetto Corsa Evo | 3058630 | `acevo_pmf_physics`, `acevo_pmf_graphics`, `acevo_pmf_static` |
| Assetto Corsa Rally | 3917090 | `acpmf_physics`, `acpmf_graphics`, `acpmf_static` |
| Le Mans Ultimate | 2399420 | `LMU_Data`, `$rFactor2SMMP_Telemetry$`, and related rFactor2 SMMP mappings |
| rFactor2 | 365960 | `$rFactor2SMMP_Telemetry$`, and related rFactor2 SMMP mappings |
| Project CARS 2 | 378860 | `$pcars2$` |
| Automobilista 2 | 1066890 | `$pcars2$` |
| RaceRoom Racing Experience | 211500 | `$R3E` |
| Euro Truck Simulator 2 | 227300 | `SHSCSTelemetry` |
| American Truck Simulator | 270880 | `SHSCSTelemetry` |

`winehub` also recognises DiRT Rally 2.0, BeamNG.drive (Windows and native Linux builds) and Wreckfest 2. These publish telemetry over UDP, so there is no shared memory to bridge. winehub only runs a stand-in process named after the game in the `--prefix` prefix, for tools that detect a game by its process name. `winecarte-run` does not handle them.

> [!WARNING]
> **rFactor2 / Le Mans Ultimate**: Do not use Winecarte alongside **rF2SharedMemoryMapPlugin_Wine**. The Wine-specific version of the plugin already writes the shared memory files under `/dev/shm`, which conflicts with Winecarte attempting to write the same files. The original windows version of the plugin (or its various forks) can be used instead.

## Installation

Download the archive from the [latest release](../../releases/latest), extract it, and place the binaries somewhere on your `PATH`, for example `~/.local/bin`:

```
cp winecarte-run winehub wine2linux.exe ~/.local/bin/
```

`winecarte-run` and `winehub` look for `wine2linux.exe` next to themselves first, then on `PATH`. Set `WINECARTE_WINE2LINUX_EXE` if it lives elsewhere.

## Exposing shared memory to Linux

Uses `winecarte-run`, set up per game. In Steam, right-click the game → Properties → Launch Options, and set:

```
winecarte-run %command%
```

If `winecarte-run` is not on your `PATH`, use its full path. Steam automatically provides `SteamAppId`, `STEAM_COMPAT_DATA_PATH`, and `STEAM_COMPAT_TOOL_PATHS`, which `winecarte-run` uses to select the game and locate the Steam runtime.

Once the game is running, the shared memory files appear under `/dev/shm`. For example, Assetto Corsa publishes:

```
/dev/shm/acpmf_physics
/dev/shm/acpmf_graphics
/dev/shm/acpmf_static
```

Any Linux application can read these files directly.

## Bridging into another Wine prefix

Uses `winehub`, which handles every supported game from one long-running process:

```
winehub --prefix /path/to/wine-prefix
```

> [!NOTE]
> `winecarte-run` is not needed when using `winehub`. Remove it from the games' Steam launch options, or leave it, in which case winehub uses its sender instead of starting its own.

`--prefix` is the prefix holding the Windows applications that should see the telemetry, and defaults to `$WINEPREFIX`. The receiver is started there with `wine` from `PATH`. Pass `--wine` if the prefix needs a different Wine build.

Leave winehub running and launch games as normal. The games need no additional Steam launch options: winehub starts the sender itself by joining the user and mount namespaces of the game's Proton container. The `/dev/shm` files are written as a result, so native Linux applications can read them in this setup too.

The receiver runs from `C:\winecarte\<game>\` in the prefix under the game's own exe name, so tools that detect a running game by process name find it.

### If the sender cannot be started

Joining the game's namespaces is refused on Steam installs where bubblewrap runs setuid, and on Flatpak Steam. winehub logs a warning and runs the receiver only, which leaves nothing to receive. Give the affected game one of these Steam launch options instead:

```
winecarte-run %command%
```

```
STEAM_COMPAT_LAUNCHER_SERVICE=proton %command%
```

With the first, winehub finds the sender from `winecarte-run` already running and leaves it alone. With the second, winehub starts the sender through `steam-runtime-launch-client`.

## Building from source

Requires Rust 1.95+ and the following cross-compilation tools:

```
# Debian/Ubuntu
sudo apt-get install musl-tools mingw-w64
```

```
# Add required targets
rustup target add x86_64-unknown-linux-musl x86_64-pc-windows-gnu
```

```
cargo build --release \
  --package winecarte-run --target x86_64-unknown-linux-musl

cargo build --release \
  --package winehub --target x86_64-unknown-linux-musl

cargo build --release \
  --package wine2linux --target x86_64-pc-windows-gnu
```

Alternatively, use `just build-all --release` if you have [just](https://github.com/casey/just) installed.

## Environment variables

| Variable | Description |
|---|---|
| `WINEPREFIX` | Default for `winehub --prefix`. |
| `WINEHUB_WINE` | Default for `winehub --wine` (default `wine`). |
| `WINECARTE_WINE2LINUX_EXE` | Override path to `wine2linux.exe`. Unnecessary if it sits next to the binary or is on `PATH`. |
| `WINECARTE_RUNTIME_LAUNCH_CLIENT` | Path to `steam-runtime-launch-client`. Auto-detected from `STEAM_COMPAT_TOOL_PATHS` if unset. |
| `WINECARTE_LOG_LEVEL` | Log level for `winecarte-run` (default `warn`) and `winehub` (default `info`). |
