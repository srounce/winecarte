use anyhow::{Context, bail};
use clap::Parser;
use log::{debug, info, warn};
use std::{
    fs::{File, OpenOptions, create_dir_all, remove_file},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    thread,
    time::Duration,
};
use thiserror::Error;

#[cfg(not(windows))]
compile_error!("wine2linux must be built for a Windows target");

use std::{io, os::windows::ffi::OsStrExt, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler},
        Memory::{
            CreateFileMappingA, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_BASIC_INFORMATION,
            MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
            UnmapViewOfFile, VirtualQuery,
        },
        Threading::{CreateEventA, OpenEventW, SetEvent, WaitForSingleObject},
    },
};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

const SYNCHRONIZE_ACCESS_MASK: u32 = 0x0010_0000;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Mirror specification in the form:
    /// - MAPPING_NAME
    /// - MAPPING_NAME|DEST_NAME
    /// - MAPPING_NAME|DEST_NAME|SIZE_BYTES
    ///
    /// MAPPING_NAME is the Win32 named file mapping to read from.
    /// DEST_NAME defaults to MAPPING_NAME if omitted.
    /// SIZE_BYTES is optional; if omitted, wine2linux queries the mapped view size.
    #[arg(long = "map", required = true, value_name = "MAPPING_NAME[|DEST_NAME[|SIZE_BYTES]]", value_parser = parse_mapping)]
    mappings: Vec<MappingArg>,

    /// Poll interval in milliseconds.
    #[arg(long, default_value_t = 5)]
    interval_ms: u64,

    /// Host destination root. Unix-style absolute paths are translated to
    /// Wine's Z: drive path at runtime.
    #[arg(long, default_value = "/dev/shm")]
    dest_root: String,

    /// Optional Win32 event name to wait on before each mirror update.
    /// If omitted, wine2linux falls back to interval polling.
    #[arg(long)]
    event: Option<String>,

    /// Run a single poll iteration and exit.
    #[arg(long, default_value_t = false)]
    once: bool,

    /// Leave mirrored output files in place on exit for debugging.
    #[arg(long, default_value_t = false)]
    keep_output_on_exit: bool,

    /// Create LMU's lock objects and use them around reads.
    #[arg(long, default_value_t = false)]
    lmu_lock: bool,
}

#[derive(Debug, Clone)]
struct MappingArg {
    mapping_name: String,
    destination_name: String,
    size: Option<usize>,
}

#[derive(Debug)]
struct MirrorTarget {
    mapping_name: String,
    destination_name: String,
    destination_host_path: PathBuf,
    destination_wine_path: PathBuf,
    size: Option<usize>,
}

struct MirrorState {
    target: MirrorTarget,
    destination_file: File,
    current: Vec<u8>,
    previous: Vec<u8>,
    source_was_available: bool,
}

struct NamedMappingHandle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for NamedMappingHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct NamedEventHandle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for NamedEventHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[repr(C)]
struct LmuLockData {
    waiters: i32,
    busy: i32,
}

struct LmuSharedMemoryLock {
    map_handle: windows_sys::Win32::Foundation::HANDLE,
    wait_event_handle: windows_sys::Win32::Foundation::HANDLE,
    data_ptr: *mut LmuLockData,
}

struct LmuSharedMemoryLockGuard<'a> {
    lock: &'a LmuSharedMemoryLock,
    held: bool,
}

impl Drop for LmuSharedMemoryLock {
    fn drop(&mut self) {
        unsafe {
            if !self.data_ptr.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.data_ptr.cast(),
                });
            }
            if !self.wait_event_handle.is_null() {
                CloseHandle(self.wait_event_handle);
            }
            if !self.map_handle.is_null() {
                CloseHandle(self.map_handle);
            }
        }
    }
}

#[derive(Error, Debug)]
enum MappingParseError {
    #[error(
        "mapping must look like MAPPING_NAME, MAPPING_NAME|DEST_NAME, or MAPPING_NAME|DEST_NAME|SIZE_BYTES"
    )]
    InvalidFormat,
    #[error("mapping name must not be empty")]
    InvalidMappingName,
    #[error("mapping destination name must not be empty or contain path separators")]
    InvalidDestinationName,
    #[error("mapping size must be greater than zero")]
    InvalidSize,
}

fn parse_mapping(value: &str) -> Result<MappingArg, String> {
    let parts = value.split('|').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len()) {
        return Err(MappingParseError::InvalidFormat.to_string());
    }

    let mapping_name = parts[0];

    if mapping_name.is_empty() {
        return Err(MappingParseError::InvalidMappingName.to_string());
    }

    let destination_name = if let Some(destination_name) = parts.get(1) {
        validate_destination_name(destination_name).map_err(|err| err.to_string())?
    } else {
        default_destination_name(mapping_name).map_err(|err| err.to_string())?
    };

    let size = if let Some(size) = parts.get(2) {
        Some(
            size.parse::<usize>()
                .ok()
                .filter(|size| *size > 0)
                .ok_or_else(|| MappingParseError::InvalidSize.to_string())?,
        )
    } else {
        None
    };

    Ok(MappingArg {
        mapping_name: mapping_name.to_string(),
        destination_name,
        size,
    })
}

fn validate_destination_name(value: &str) -> Result<String, MappingParseError> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(MappingParseError::InvalidDestinationName);
    }

    if value.contains('/') || value.contains('\\') {
        return Err(MappingParseError::InvalidDestinationName);
    }

    Ok(value.to_string())
}

fn default_destination_name(mapping_name: &str) -> Result<String, MappingParseError> {
    mapping_name
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .ok_or(MappingParseError::InvalidDestinationName)
        .and_then(validate_destination_name)
}

fn wine_path_from_host_root(root: &str) -> PathBuf {
    if looks_like_unix_absolute(root) {
        unix_path_to_wine(root)
    } else {
        PathBuf::from(root)
    }
}

fn looks_like_unix_absolute(path: &str) -> bool {
    path.starts_with('/')
}

fn unix_path_to_wine(path: &str) -> PathBuf {
    let trimmed = path.trim_start_matches('/');
    let mut wine_path = String::from("Z:\\");
    if !trimmed.is_empty() {
        wine_path.push_str(&trimmed.replace('/', "\\"));
    }
    PathBuf::from(wine_path)
}

fn build_targets(args: &Args) -> Vec<MirrorTarget> {
    let destination_host_root = PathBuf::from(&args.dest_root);
    let destination_wine_root = wine_path_from_host_root(&args.dest_root);

    args.mappings
        .iter()
        .map(|mapping| MirrorTarget {
            mapping_name: mapping.mapping_name.clone(),
            destination_name: mapping.destination_name.clone(),
            destination_host_path: destination_host_root.join(&mapping.destination_name),
            destination_wine_path: destination_wine_root.join(&mapping.destination_name),
            size: mapping.size,
        })
        .collect()
}

fn ensure_destination_file(path: &Path, size: usize) -> anyhow::Result<File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent).with_context(|| {
            format!(
                "failed to create destination directory {}",
                parent.display()
            )
        })?;
    }

    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open destination file {}", path.display()))?;

    file.set_len(size as u64)
        .with_context(|| format!("failed to size destination file {}", path.display()))?;

    Ok(file)
}

fn initialize_state(target: MirrorTarget) -> anyhow::Result<MirrorState> {
    let size = target
        .size
        .context("mirror target size must be resolved before initialization")?;
    let destination_file = ensure_destination_file(&target.destination_wine_path, size)?;

    Ok(MirrorState {
        previous: vec![0; size],
        current: vec![0; size],
        destination_file,
        source_was_available: false,
        target,
    })
}

fn cleanup_states(states: Vec<MirrorState>, keep_output_on_exit: bool) {
    for state in states {
        let destination_path = state.target.destination_wine_path.clone();
        drop(state.destination_file);

        if keep_output_on_exit {
            info!("keeping destination file {}", destination_path.display());
            continue;
        }

        match remove_file(&destination_path) {
            Ok(()) => {
                info!("removed destination file {}", destination_path.display());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                warn!(
                    "failed to remove destination file {} ({error})",
                    destination_path.display()
                );
            }
        }
    }
}

impl LmuSharedMemoryLock {
    fn initialize() -> anyhow::Result<Self> {
        const LOCK_DATA_NAME: &[u8] = b"LMU_SharedMemoryLockData\0";
        const LOCK_EVENT_NAME: &[u8] = b"LMU_SharedMemoryLockEvent\0";

        let map_handle = unsafe {
            CreateFileMappingA(
                windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                std::mem::size_of::<LmuLockData>() as u32,
                LOCK_DATA_NAME.as_ptr(),
            )
        };
        if map_handle.is_null() {
            return Err(io::Error::last_os_error()).context("failed to create LMU lock mapping");
        }
        let lock_already_exists = io::Error::last_os_error().raw_os_error()
            == Some(windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32);

        let data_ptr = unsafe {
            MapViewOfFile(
                map_handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<LmuLockData>(),
            )
        }
        .Value
        .cast::<LmuLockData>();
        if data_ptr.is_null() {
            unsafe {
                CloseHandle(map_handle);
            }
            return Err(io::Error::last_os_error()).context("failed to map LMU lock data");
        }

        let wait_event_handle =
            unsafe { CreateEventA(std::ptr::null(), 0, 0, LOCK_EVENT_NAME.as_ptr()) };
        if wait_event_handle.is_null() {
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: data_ptr.cast(),
                });
                CloseHandle(map_handle);
            }
            return Err(io::Error::last_os_error()).context("failed to create LMU lock event");
        }

        let lock = Self {
            map_handle,
            wait_event_handle,
            data_ptr,
        };

        if !lock_already_exists {
            lock.reset();
        }

        Ok(lock)
    }

    fn reset(&self) {
        self.waiters().store(0, Ordering::Release);
        self.busy().store(0, Ordering::Release);
    }

    fn busy(&self) -> &AtomicI32 {
        unsafe { &*(std::ptr::addr_of!((*self.data_ptr).busy).cast::<AtomicI32>()) }
    }

    fn waiters(&self) -> &AtomicI32 {
        unsafe { &*(std::ptr::addr_of!((*self.data_ptr).waiters).cast::<AtomicI32>()) }
    }

    fn lock(&self, timeout_ms: u32) -> anyhow::Result<LmuSharedMemoryLockGuard<'_>> {
        const MAX_SPINS: usize = 4000;

        for _ in 0..MAX_SPINS {
            if self
                .busy()
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(LmuSharedMemoryLockGuard {
                    lock: self,
                    held: true,
                });
            }
            std::hint::spin_loop();
        }

        self.waiters().fetch_add(1, Ordering::AcqRel);
        loop {
            if self
                .busy()
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.waiters().fetch_sub(1, Ordering::AcqRel);
                return Ok(LmuSharedMemoryLockGuard {
                    lock: self,
                    held: true,
                });
            }

            let result = unsafe { WaitForSingleObject(self.wait_event_handle, timeout_ms) };
            match result {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => {
                    if shutdown_requested() {
                        self.waiters().fetch_sub(1, Ordering::AcqRel);
                        bail!("shutdown requested while waiting for LMU lock");
                    }
                }
                _ => {
                    self.waiters().fetch_sub(1, Ordering::AcqRel);
                    return Err(io::Error::last_os_error())
                        .context("failed while waiting for LMU lock event");
                }
            }
        }
    }

    fn unlock(&self) -> anyhow::Result<()> {
        self.busy().store(0, Ordering::Release);
        if self.waiters().load(Ordering::Acquire) > 0 {
            let signaled = unsafe { SetEvent(self.wait_event_handle) };
            if signaled == 0 {
                return Err(io::Error::last_os_error()).context("failed to signal LMU lock event");
            }
        }

        Ok(())
    }
}

impl Drop for LmuSharedMemoryLockGuard<'_> {
    fn drop(&mut self) {
        if self.held {
            let _ = self.lock.unlock();
            self.held = false;
        }
    }
}

fn open_named_mapping(mapping_name: &str) -> anyhow::Result<NamedMappingHandle> {
    let wide_name = encode_wide_null(mapping_name);
    let mapping = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide_name.as_ptr()) };
    if mapping.is_null() {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("failed to open named mapping {mapping_name}"));
    }

    Ok(NamedMappingHandle(mapping))
}

fn open_named_event(event_name: &str) -> anyhow::Result<NamedEventHandle> {
    let wide_name = encode_wide_null(event_name);
    let event = unsafe { OpenEventW(SYNCHRONIZE_ACCESS_MASK, 0, wide_name.as_ptr()) };
    if event.is_null() {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("failed to open named event {event_name}"));
    }

    Ok(NamedEventHandle(event))
}

fn encode_wide_null(value: &str) -> Vec<u16> {
    let mut wide = std::ffi::OsStr::new(value)
        .encode_wide()
        .collect::<Vec<_>>();
    wide.push(0);
    wide
}

fn detect_mapping_size(mapping_name: &str) -> anyhow::Result<usize> {
    let mapping = open_named_mapping(mapping_name)?;
    let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, 0) };
    if view.Value.is_null() {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!("failed to map named mapping {mapping_name} for size detection")
        });
    }

    let mut info = std::mem::MaybeUninit::<MEMORY_BASIC_INFORMATION>::zeroed();
    let queried = unsafe {
        VirtualQuery(
            view.Value,
            info.as_mut_ptr(),
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };

    unsafe {
        UnmapViewOfFile(view);
    }

    if queried == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!("failed to query mapped region size for named mapping {mapping_name}")
        });
    }

    let info = unsafe { info.assume_init() };
    if info.RegionSize == 0 {
        bail!("named mapping {mapping_name} reported a zero-sized mapped region");
    }

    Ok(info.RegionSize)
}

fn resolve_target_sizes(targets: &mut [MirrorTarget]) -> anyhow::Result<()> {
    for target in targets {
        if target.size.is_none() {
            let size = detect_mapping_size(&target.mapping_name)?;
            info!(
                "detected mapping size for {}: {} bytes",
                target.mapping_name, size
            );
            target.size = Some(size);
        }
    }

    Ok(())
}

fn wait_for_targets(targets: &[MirrorTarget], interval: Duration) {
    for target in targets {
        let mut logged_waiting = false;
        loop {
            if shutdown_requested() {
                return;
            }

            match open_named_mapping(&target.mapping_name) {
                Ok(_mapping) => {
                    info!("source is ready: {}", target.mapping_name);
                    break;
                }
                Err(error) => {
                    if !logged_waiting {
                        info!(
                            "waiting for source mapping {} ({error:#})",
                            target.mapping_name
                        );
                        logged_waiting = true;
                    }
                    thread::sleep(interval);
                }
            }
        }
    }
}

fn wait_for_event_ready(event_name: &str, interval: Duration) {
    let mut logged_waiting = false;
    loop {
        if shutdown_requested() {
            return;
        }

        match open_named_event(event_name) {
            Ok(_event) => {
                info!("event is ready: {event_name}");
                return;
            }
            Err(error) => {
                if !logged_waiting {
                    info!("waiting for event {event_name} ({error:#})");
                    logged_waiting = true;
                }
                thread::sleep(interval);
            }
        }
    }
}

fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

unsafe extern "system" fn console_ctrl_handler(control_type: u32) -> i32 {
    match control_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
            1
        }
        _ => 0,
    }
}

fn install_console_ctrl_handler() -> anyhow::Result<()> {
    let installed = unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), 1) };
    if installed == 0 {
        return Err(io::Error::last_os_error())
            .context("failed to install console control handler");
    }

    Ok(())
}

fn read_source_exact(mapping_name: &str, buffer: &mut [u8]) -> anyhow::Result<()> {
    let mapping = open_named_mapping(mapping_name)?;

    let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, buffer.len()) };
    if view.Value.is_null() {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "failed to map {} bytes from named mapping {mapping_name}",
                buffer.len()
            )
        });
    }

    unsafe {
        ptr::copy_nonoverlapping(view.Value.cast::<u8>(), buffer.as_mut_ptr(), buffer.len());
        UnmapViewOfFile(view);
    }

    Ok(())
}

fn wait_for_event_signal(event_name: &str, interval: Duration) -> anyhow::Result<bool> {
    let event = open_named_event(event_name)?;
    let timeout_ms = interval.as_millis().clamp(1, u32::MAX as u128) as u32;

    loop {
        if shutdown_requested() {
            return Ok(false);
        }

        let result = unsafe { WaitForSingleObject(event.0, timeout_ms) };
        match result {
            WAIT_OBJECT_0 | WAIT_TIMEOUT => return Ok(true),
            _ => {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("failed while waiting for event {event_name}"));
            }
        }
    }
}

fn copy_if_changed(
    state: &mut MirrorState,
    lmu_lock: Option<&LmuSharedMemoryLock>,
    interval: Duration,
) -> anyhow::Result<bool> {
    let _lock_guard = if let Some(lock) = lmu_lock {
        let timeout_ms = interval.as_millis().clamp(1, u32::MAX as u128) as u32;
        Some(lock.lock(timeout_ms)?)
    } else {
        None
    };

    let read_result = read_source_exact(&state.target.mapping_name, &mut state.current);

    match read_result {
        Ok(()) => {
            if !state.source_was_available {
                info!("source became available: {}", state.target.mapping_name);
                state.source_was_available = true;
            }
        }
        Err(error) => {
            if state.source_was_available {
                warn!(
                    "source became unavailable: {} ({error:#})",
                    state.target.mapping_name
                );
                state.source_was_available = false;
            } else {
                debug!(
                    "source still unavailable: {} ({error:#})",
                    state.target.mapping_name
                );
            }
            return Ok(false);
        }
    }

    if state.current == state.previous {
        return Ok(false);
    }

    state
        .destination_file
        .seek(SeekFrom::Start(0))
        .with_context(|| {
            format!(
                "failed to rewind destination file {}",
                state.target.destination_wine_path.display()
            )
        })?;
    state
        .destination_file
        .write_all(&state.current)
        .with_context(|| {
            format!(
                "failed to write destination file {}",
                state.target.destination_wine_path.display()
            )
        })?;
    state.destination_file.flush().with_context(|| {
        format!(
            "failed to flush destination file {}",
            state.target.destination_wine_path.display()
        )
    })?;

    state.previous.copy_from_slice(&state.current);

    Ok(true)
}

fn main() -> anyhow::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_env("WINECARTE_LOG_LEVEL")
        .format_level(true)
        .format_module_path(true)
        .format_target(true)
        .try_init()?;

    let args = Args::parse();
    install_console_ctrl_handler()?;
    let lmu_lock = if args.lmu_lock {
        let lock = LmuSharedMemoryLock::initialize()?;
        info!("LMU compatibility lock initialized");
        Some(lock)
    } else {
        None
    };

    let targets = build_targets(&args);
    if targets.is_empty() {
        bail!("at least one mapping must be provided");
    }

    info!(
        "starting mirror loop for {} mapping(s) with interval={}ms",
        targets.len(),
        args.interval_ms
    );

    for target in &targets {
        info!(
            "mapping {} -> {} (host {}, name {}) [{} bytes]",
            target.mapping_name,
            target.destination_wine_path.display(),
            target.destination_host_path.display(),
            target.destination_name,
            target
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "auto".to_string())
        );
    }

    let interval = Duration::from_millis(args.interval_ms);
    let mut targets = targets;
    wait_for_targets(&targets, interval);
    if shutdown_requested() {
        info!("shutdown requested before sources became ready");
        return Ok(());
    }
    if let Some(event_name) = args.event.as_deref() {
        wait_for_event_ready(event_name, interval);
        if shutdown_requested() {
            info!("shutdown requested before event became ready");
            return Ok(());
        }
    }
    resolve_target_sizes(&mut targets)?;

    let mut states = targets
        .into_iter()
        .map(initialize_state)
        .collect::<anyhow::Result<Vec<_>>>()?;

    let result = (|| -> anyhow::Result<()> {
        loop {
            if let Some(event_name) = args.event.as_deref() {
                if !wait_for_event_signal(event_name, interval)? {
                    break;
                }
            }

            let mut copied_count = 0usize;

            for state in &mut states {
                if copy_if_changed(state, lmu_lock.as_ref(), interval)? {
                    copied_count += 1;
                }
            }

            debug!("poll iteration complete; updated {copied_count} mapping(s)");

            if args.once || shutdown_requested() {
                break;
            }

            if args.event.is_none() {
                thread::sleep(interval);
            }
        }

        Ok(())
    })();

    cleanup_states(states, args.keep_output_on_exit);

    result
}

#[cfg(test)]
mod tests {
    use super::{parse_mapping, unix_path_to_wine, validate_destination_name};

    #[test]
    fn parses_mapping_spec() {
        let mapping = parse_mapping(r"Local\acpmf_physics|telemetry|4096").unwrap();
        assert_eq!(mapping.mapping_name, r"Local\acpmf_physics");
        assert_eq!(mapping.destination_name, "telemetry");
        assert_eq!(mapping.size, Some(4096));
    }

    #[test]
    fn parses_mapping_spec_without_size() {
        let mapping = parse_mapping(r"LMU_Data|telemetry").unwrap();
        assert_eq!(mapping.mapping_name, "LMU_Data");
        assert_eq!(mapping.destination_name, "telemetry");
        assert_eq!(mapping.size, None);
    }

    #[test]
    fn defaults_destination_name_from_mapping_name() {
        let mapping = parse_mapping(r"Local\LMU_Data").unwrap();
        assert_eq!(mapping.mapping_name, r"Local\LMU_Data");
        assert_eq!(mapping.destination_name, "LMU_Data");
        assert_eq!(mapping.size, None);
    }

    #[test]
    fn rejects_destination_paths() {
        assert!(validate_destination_name("nested/name").is_err());
        assert!(validate_destination_name(r"nested\name").is_err());
        assert!(validate_destination_name("..").is_err());
    }

    #[test]
    fn rejects_extra_mapping_segments() {
        assert!(parse_mapping("src|dest|123|extra").is_err());
    }

    #[test]
    fn translates_unix_root_to_wine_z_drive() {
        assert_eq!(
            unix_path_to_wine("/dev/shm").to_string_lossy(),
            r"Z:\dev\shm"
        );
    }
}
