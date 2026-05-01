# winecarte

Racing simulators publish live telemetry — physics, graphics state, lap timing — through Win32 named shared memory. When these games run under Proton on Linux, that shared memory is confined to the Wine environment and unreachable by native Linux applications such as dashboards, overlays, and telemetry recorders.

winecarte bridges that gap by mirroring Win32 named mappings into Linux shared memory files under `/dev/shm`, making them available to any Linux process as though the game were running natively.

## How it works

winecarte consists of two binaries that work in tandem:

**`wine2linux.exe`** runs inside the Wine/Proton environment alongside the game. It opens the Win32 named file mappings created by the game and periodically copies their contents into Linux files (defaulting to `/dev/shm`). It is cross-compiled to a Windows executable so it can call Win32 APIs directly.

**`winecarte-run`** is a native Linux binary that acts as a Steam compatibility tool. Steam calls it in place of the game launcher. It spawns the real launcher, watches `/proc` until the game process appears, then uses `steam-runtime-launch-client` to launch `wine2linux.exe` inside the running Proton instance. When the game exits, it tears `wine2linux.exe` down cleanly.

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

## Supported games

| Game | Steam App ID | Shared memory files |
|---|---|---|
| Assetto Corsa | 244210 | `acpmf_physics`, `acpmf_graphics`, `acpmf_static` |
| Assetto Corsa Competizione | 805550 | `acpmf_physics`, `acpmf_graphics`, `acpmf_static` |
| Assetto Corsa Evo | 3058630 | `acpmf_physics`, `acpmf_graphics`, `acpmf_static` |
| Le Mans Ultimate | 2399420 | `LMU_Data`, `$rFactor2SMMP_Telemetry$`, and related rFactor2 mappings |
| Project CARS 2 | 378860 | `$pcars2$` |
| Automobilista 2 | 1066890 | `$pcars2$` |

## Usage

### 1. Get the binaries

Download `winecarte-run` and `wine2linux.exe` from the [latest release](../../releases/latest) and extract the archive.

### 2. Install both binaries to PATH

The simplest setup is to place both binaries somewhere on your `PATH`, for example `~/.local/bin`:

```
cp winecarte-run ~/.local/bin/
cp wine2linux.exe ~/.local/bin/
```

`winecarte-run` searches `PATH` for `wine2linux.exe` at launch time, so with both installed this way no further configuration is needed.

### 3. Set Steam launch options

In Steam, right-click the game → Properties → Launch Options, and set the following launch command.

If both binaries are on your `PATH`:

```
winecarte-run %command%
```

If they are not on your `PATH`, use full paths and point `WINECARTE_WINE2LINUX_EXE` at `wine2linux.exe`:

```
WINECARTE_WINE2LINUX_EXE=/path/to/wine2linux.exe /path/to/winecarte-run %command%
```

Steam automatically provides `SteamAppId`, `STEAM_COMPAT_DATA_PATH`, and `STEAM_COMPAT_TOOL_PATHS`, which `winecarte-run` uses to select the right handler and locate the Steam runtime.

### 4. Read the data

Once the game is running, the shared memory files appear under `/dev/shm`. For example, Assetto Corsa publishes:

```
/dev/shm/acpmf_physics
/dev/shm/acpmf_graphics
/dev/shm/acpmf_static
```

Any Linux application can read these files directly.

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
  --package wine2linux --target x86_64-pc-windows-gnu
```

Alternatively, use `just build-all --release` if you have [just](https://github.com/casey/just) installed.

## Environment variables

| Variable | Description |
|---|---|
| `WINECARTE_WINE2LINUX_EXE` | Override path to `wine2linux.exe`. Unnecessary if it is on `PATH`. |
| `WINECARTE_RUNTIME_LAUNCH_CLIENT` | Path to `steam-runtime-launch-client`. Auto-detected from `STEAM_COMPAT_TOOL_PATHS` if unset. |
| `WINECARTE_LOG_LEVEL` | Log level for `winecarte-run`. Defaults to `warn`. |
