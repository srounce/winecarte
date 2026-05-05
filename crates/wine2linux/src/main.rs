use anyhow::{Context, bail};
use clap::Parser;
use log::{debug, info, warn, trace};
use std::{
    fs::{File, OpenOptions, create_dir_all, remove_file},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    thread,
    time::Duration,
};
use thiserror::Error;

#[cfg(not(windows))]
compile_error!("wine2linux must be built for a Windows target");

use std::{io, os::windows::{ffi::OsStrExt, io::AsRawHandle}, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler},
        Memory::{
            CreateFileMappingA, CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ,
            MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
            PAGE_READWRITE, UnmapViewOfFile, VirtualQuery,
        },
        Threading::{CreateEventA, OpenEventW, SetEvent, WaitForSingleObject},
    },
};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

const SYNCHRONIZE_ACCESS_MASK: u32 = 0x0010_0000;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Deprecated: use --from-wine instead. This option will be removed in a
    /// future version.
    /// Mirror specification in the form:
    /// - MAPPING_NAME
    /// - MAPPING_NAME|DEST_NAME
    /// - MAPPING_NAME|DEST_NAME|SIZE_BYTES
    ///
    /// MAPPING_NAME is the Win32 named file mapping to read from.
    /// DEST_NAME defaults to MAPPING_NAME if omitted.
    /// SIZE_BYTES is optional; if omitted, wine2linux queries the mapped view size.
    #[arg(long = "map", value_name = "MAPPING_NAME[|DEST_NAME[|SIZE_BYTES]]", value_parser = parse_mapping)]
    mappings: Vec<MappingArg>,

    /// Mirror data from a Wine named mapping into Linux shared memory.
    /// Forms:
    /// - WINE_MAPPING
    /// - WINE_MAPPING|LINUX_PATH
    /// - WINE_MAPPING|LINUX_PATH|SIZE_BYTES
    #[arg(long = "from-wine", value_name = "WINE_MAPPING[|LINUX_PATH[|SIZE_BYTES]]", value_parser = parse_from_wine)]
    from_wine: Vec<FromWineArg>,

    /// Mirror data from Linux shared memory into a Wine named mapping.
    /// Forms:
    /// - LINUX_PATH
    /// - LINUX_PATH|WINE_MAPPING
    /// - LINUX_PATH|WINE_MAPPING|SIZE_BYTES
    #[arg(long = "from-linux", value_name = "LINUX_PATH[|WINE_MAPPING[|SIZE_BYTES]]", value_parser = parse_from_linux)]
    from_linux: Vec<FromLinuxArg>,

    /// Poll interval in milliseconds.
    #[arg(long, default_value_t = 16)]
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

#[derive(Debug, Clone)]
struct FromWineArg {
    mapping_name: String,
    linux_path: String,
    size: Option<usize>,
}

#[derive(Debug, Clone)]
struct FromLinuxArg {
    linux_path: String,
    mapping_name: String,
    size: Option<usize>,
}

#[derive(Debug)]
struct FromWineTarget {
    mapping_name: String,
    linux_path: String,
    destination_host_path: String,
    destination_wine_path: PathBuf,
    size: Option<usize>,
}

struct FromWineState {
    target: FromWineTarget,
    destination_mapping: Option<DestinationFileMapping>,
    current: Vec<u8>,
    previous: Vec<u8>,
    source_was_available: bool,
    waiting_logged: bool,
}

#[derive(Debug)]
struct FromLinuxTarget {
    source_host_path: String,
    source_wine_path: PathBuf,
    mapping_name: String,
    size: Option<usize>,
}

struct FromLinuxState {
    target: FromLinuxTarget,
    destination_mapping: WritableMapping,
    current: Vec<u8>,
    previous: Vec<u8>,
    source_was_available: bool,
    source_size_mismatch: bool,
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

struct WritableMapping {
    handle: windows_sys::Win32::Foundation::HANDLE,
    view: *mut u8,
}

impl Drop for WritableMapping {
    fn drop(&mut self) {
        unsafe {
            if !self.view.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.view.cast(),
                });
            }
            if !self.handle.is_null() {
                CloseHandle(self.handle);
            }
        }
    }
}

struct DestinationFileMapping {
    _file: File,
    map_handle: windows_sys::Win32::Foundation::HANDLE,
    view: *mut u8,
    size: usize,
}

impl Drop for DestinationFileMapping {
    fn drop(&mut self) {
        unsafe {
            if !self.view.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.view.cast(),
                });
            }
            if !self.map_handle.is_null() {
                CloseHandle(self.map_handle);
            }
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
enum ArgParseError {
    #[error("mapping size must be greater than zero")]
    InvalidSize,
    #[error("wine mapping name must not be empty")]
    InvalidMappingName,
    #[error(
        "linux shared memory path must be relative, non-empty, and must not contain '..' or backslashes"
    )]
    InvalidLinuxPath,
    #[error(
        "deprecated --map must look like MAPPING_NAME, MAPPING_NAME|DEST_NAME, or MAPPING_NAME|DEST_NAME|SIZE_BYTES"
    )]
    InvalidDeprecatedMapFormat,
    #[error(
        "--from-wine must look like WINE_MAPPING, WINE_MAPPING|LINUX_PATH, or WINE_MAPPING|LINUX_PATH|SIZE_BYTES"
    )]
    InvalidFromWineFormat,
    #[error(
        "--from-linux must look like LINUX_PATH, LINUX_PATH|WINE_MAPPING, or LINUX_PATH|WINE_MAPPING|SIZE_BYTES"
    )]
    InvalidFromLinuxFormat,
}

fn parse_positive_size(value: &str) -> Result<usize, ArgParseError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|size| *size > 0)
        .ok_or(ArgParseError::InvalidSize)
}

fn validate_mapping_name(value: &str) -> Result<String, ArgParseError> {
    if value.is_empty() {
        return Err(ArgParseError::InvalidMappingName);
    }

    Ok(value.to_string())
}

fn validate_linux_path(value: &str) -> Result<String, ArgParseError> {
    if value.is_empty() || value.starts_with('/') || value.contains('\\') {
        return Err(ArgParseError::InvalidLinuxPath);
    }

    let mut saw_segment = false;
    for segment in value.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(ArgParseError::InvalidLinuxPath);
        }
        saw_segment = true;
    }

    if !saw_segment {
        return Err(ArgParseError::InvalidLinuxPath);
    }

    Ok(value.to_string())
}

fn default_destination_name(mapping_name: &str) -> Result<String, ArgParseError> {
    mapping_name
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .ok_or(ArgParseError::InvalidLinuxPath)
        .map(ToString::to_string)
}

fn default_linux_path_from_mapping(mapping_name: &str) -> Result<String, ArgParseError> {
    validate_linux_path(&mapping_name.replace('\\', "/"))
}

fn default_mapping_name_from_linux_path(linux_path: &str) -> Result<String, ArgParseError> {
    linux_path
        .rsplit('/')
        .next()
        .ok_or(ArgParseError::InvalidLinuxPath)
        .and_then(validate_mapping_name)
}

fn parse_mapping(value: &str) -> Result<MappingArg, String> {
    let parts = value.split('|').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len()) {
        return Err(ArgParseError::InvalidDeprecatedMapFormat.to_string());
    }

    let mapping_name = validate_mapping_name(parts[0]).map_err(|err| err.to_string())?;
    let destination_name = if let Some(destination_name) = parts.get(1) {
        validate_linux_path(destination_name)
            .map_err(|_| ArgParseError::InvalidDeprecatedMapFormat.to_string())?
    } else {
        default_destination_name(&mapping_name).map_err(|err| err.to_string())?
    };
    let size = parts
        .get(2)
        .map(|size| parse_positive_size(size).map_err(|err| err.to_string()))
        .transpose()?;

    Ok(MappingArg {
        mapping_name,
        destination_name,
        size,
    })
}

fn parse_from_wine(value: &str) -> Result<FromWineArg, String> {
    let parts = value.split('|').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len()) {
        return Err(ArgParseError::InvalidFromWineFormat.to_string());
    }

    let mapping_name = validate_mapping_name(parts[0]).map_err(|err| err.to_string())?;
    let linux_path = if let Some(linux_path) = parts.get(1) {
        validate_linux_path(linux_path).map_err(|err| err.to_string())?
    } else {
        default_linux_path_from_mapping(&mapping_name).map_err(|err| err.to_string())?
    };
    let size = parts
        .get(2)
        .map(|size| parse_positive_size(size).map_err(|err| err.to_string()))
        .transpose()?;

    Ok(FromWineArg {
        mapping_name,
        linux_path,
        size,
    })
}

fn parse_from_linux(value: &str) -> Result<FromLinuxArg, String> {
    let parts = value.split('|').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len()) {
        return Err(ArgParseError::InvalidFromLinuxFormat.to_string());
    }

    let linux_path = validate_linux_path(parts[0]).map_err(|err| err.to_string())?;
    let mapping_name = if let Some(mapping_name) = parts.get(1) {
        validate_mapping_name(mapping_name).map_err(|err| err.to_string())?
    } else {
        default_mapping_name_from_linux_path(&linux_path).map_err(|err| err.to_string())?
    };
    let size = parts
        .get(2)
        .map(|size| parse_positive_size(size).map_err(|err| err.to_string()))
        .transpose()?;

    Ok(FromLinuxArg {
        linux_path,
        mapping_name,
        size,
    })
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

fn linux_host_path(root: &str, linux_path: &str) -> String {
    if root.ends_with('/') {
        format!("{root}{linux_path}")
    } else {
        format!("{root}/{linux_path}")
    }
}

fn linux_wine_path(root: &Path, linux_path: &str) -> PathBuf {
    let mut root = root.to_string_lossy().into_owned();
    if !root.ends_with('\\') && !root.ends_with('/') {
        root.push('\\');
    }
    root.push_str(&linux_path.replace('/', "\\"));
    PathBuf::from(root)
}

fn build_from_wine_targets(args: &Args) -> Vec<FromWineTarget> {
    let destination_wine_root = wine_path_from_host_root(&args.dest_root);

    let deprecated_targets = args.mappings.iter().map(|mapping| FromWineTarget {
        mapping_name: mapping.mapping_name.clone(),
        linux_path: mapping.destination_name.clone(),
        destination_host_path: linux_host_path(&args.dest_root, &mapping.destination_name),
        destination_wine_path: linux_wine_path(&destination_wine_root, &mapping.destination_name),
        size: mapping.size,
    });

    let new_targets = args.from_wine.iter().map(|mapping| FromWineTarget {
        mapping_name: mapping.mapping_name.clone(),
        linux_path: mapping.linux_path.clone(),
        destination_host_path: linux_host_path(&args.dest_root, &mapping.linux_path),
        destination_wine_path: linux_wine_path(&destination_wine_root, &mapping.linux_path),
        size: mapping.size,
    });

    deprecated_targets.chain(new_targets).collect()
}

fn build_from_linux_targets(args: &Args) -> Vec<FromLinuxTarget> {
    let source_wine_root = wine_path_from_host_root(&args.dest_root);

    args.from_linux
        .iter()
        .map(|mapping| FromLinuxTarget {
            source_host_path: linux_host_path(&args.dest_root, &mapping.linux_path),
            source_wine_path: linux_wine_path(&source_wine_root, &mapping.linux_path),
            mapping_name: mapping.mapping_name.clone(),
            size: mapping.size,
        })
        .collect()
}

fn ensure_destination_mapping(path: &Path, size: usize) -> anyhow::Result<DestinationFileMapping> {
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

    let file_handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let size_u64 = size as u64;
    let map_handle = unsafe {
        CreateFileMappingA(
            file_handle,
            std::ptr::null(),
            PAGE_READWRITE,
            (size_u64 >> 32) as u32,
            size_u64 as u32,
            std::ptr::null(),
        )
    };
    if map_handle.is_null() {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("failed to create file mapping for {}", path.display()));
    }

    let view = unsafe { MapViewOfFile(map_handle, FILE_MAP_ALL_ACCESS, 0, 0, size) }.Value;
    if view.is_null() {
        unsafe { CloseHandle(map_handle); }
        return Err(io::Error::last_os_error())
            .with_context(|| format!("failed to map view for {}", path.display()));
    }

    Ok(DestinationFileMapping {
        _file: file,
        map_handle,
        view: view.cast(),
        size,
    })
}

fn initialize_from_wine_state(target: FromWineTarget) -> anyhow::Result<FromWineState> {
    Ok(FromWineState {
        previous: Vec::new(),
        current: Vec::new(),
        destination_mapping: None,
        source_was_available: false,
        waiting_logged: false,
        target,
    })
}

fn cleanup_from_wine_states(states: Vec<FromWineState>, keep_output_on_exit: bool) {
    for state in states {
        let destination_path = state.target.destination_wine_path.clone();
        drop(state.destination_mapping);

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

fn create_writable_mapping(mapping_name: &str, size: usize) -> anyhow::Result<WritableMapping> {
    let wide_name = encode_wide_null(mapping_name);
    let size_u64 = size as u64;
    let mapping = unsafe {
        CreateFileMappingW(
            windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (size_u64 >> 32) as u32,
            size_u64 as u32,
            wide_name.as_ptr(),
        )
    };
    if mapping.is_null() {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("failed to create writable named mapping {mapping_name}"));
    }

    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, size) }.Value;
    if view.is_null() {
        unsafe {
            CloseHandle(mapping);
        }
        return Err(io::Error::last_os_error())
            .with_context(|| format!("failed to map writable named mapping {mapping_name}"));
    }

    Ok(WritableMapping {
        handle: mapping,
        view: view.cast(),
    })
}

fn initialize_from_linux_state(target: FromLinuxTarget) -> anyhow::Result<FromLinuxState> {
    let size = target
        .size
        .context("from-linux target size must be resolved before initialization")?;
    let destination_mapping = create_writable_mapping(&target.mapping_name, size)?;

    Ok(FromLinuxState {
        target,
        destination_mapping,
        current: vec![0; size],
        previous: vec![0; size],
        source_was_available: false,
        source_size_mismatch: false,
    })
}

fn linux_source_size(path: &Path) -> anyhow::Result<usize> {
    let len = std::fs::metadata(path)
        .with_context(|| {
            format!(
                "failed to stat Linux shared memory source {}",
                path.display()
            )
        })?
        .len();

    if len == 0 {
        bail!("Linux shared memory source {} is empty", path.display());
    }

    usize::try_from(len)
        .with_context(|| format!("Linux shared memory source {} is too large", path.display()))
}

fn resolve_from_linux_target_sizes(targets: &mut [FromLinuxTarget], interval: Duration) {
    for target in targets {
        if target.size.is_some() {
            continue;
        }

        let mut logged_waiting = false;
        loop {
            if shutdown_requested() {
                return;
            }

            match linux_source_size(&target.source_wine_path) {
                Ok(size) => {
                    info!(
                        "detected source size for {}: {} bytes",
                        target.source_host_path, size
                    );
                    target.size = Some(size);
                    break;
                }
                Err(error) => {
                    if !logged_waiting {
                        info!(
                            "waiting for Linux shared memory source {} ({error:#})",
                            target.source_host_path
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

fn read_from_wine_exact(mapping_name: &str, buffer: &mut [u8]) -> anyhow::Result<()> {
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

fn read_from_linux_exact(path: &Path, buffer: &mut [u8]) -> anyhow::Result<()> {
    let size = linux_source_size(path)?;
    if size != buffer.len() {
        bail!(
            "Linux shared memory source {} has size {} but expected {}",
            path.display(),
            size,
            buffer.len()
        );
    }

    let mut file = File::open(path).with_context(|| {
        format!(
            "failed to open Linux shared memory source {}",
            path.display()
        )
    })?;
    file.read_exact(buffer).with_context(|| {
        format!(
            "failed to read Linux shared memory source {}",
            path.display()
        )
    })?;

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

fn ensure_from_wine_state_ready(state: &mut FromWineState) -> anyhow::Result<bool> {
    if state.destination_mapping.is_some() {
        return Ok(true);
    }

    match open_named_mapping(&state.target.mapping_name) {
        Ok(_mapping) => {
            if state.waiting_logged {
                info!("source is ready: {}", state.target.mapping_name);
                state.waiting_logged = false;
            }
        }
        Err(error) => {
            if !state.waiting_logged {
                info!(
                    "waiting for source mapping {} ({error:#})",
                    state.target.mapping_name
                );
                state.waiting_logged = true;
            }
            return Ok(false);
        }
    }

    if state.target.size.is_none() {
        let size = detect_mapping_size(&state.target.mapping_name)?;
        info!(
            "detected mapping size for {}: {} bytes",
            state.target.mapping_name, size
        );
        state.target.size = Some(size);
    }

    let size = state
        .target
        .size
        .context("from-wine target size must be resolved before initialization")?;
    state.destination_mapping = Some(ensure_destination_mapping(
        &state.target.destination_wine_path,
        size,
    )?);
    state.current = vec![0; size];
    state.previous = vec![0; size];

    Ok(true)
}

fn copy_from_wine_if_changed(
    state: &mut FromWineState,
    lmu_lock: Option<&LmuSharedMemoryLock>,
    interval: Duration,
) -> anyhow::Result<bool> {
    if !ensure_from_wine_state_ready(state)? {
        return Ok(false);
    }

    let _lock_guard = if let Some(lock) = lmu_lock {
        let timeout_ms = interval.as_millis().clamp(1, u32::MAX as u128) as u32;
        Some(lock.lock(timeout_ms)?)
    } else {
        None
    };

    let read_result = read_from_wine_exact(&state.target.mapping_name, &mut state.current);

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

    let mapping = state
        .destination_mapping
        .as_ref()
        .context("from-wine destination mapping not initialized")?;
    unsafe {
        ptr::copy_nonoverlapping(state.current.as_ptr(), mapping.view, mapping.size);
    }

    state.previous.copy_from_slice(&state.current);

    Ok(true)
}

fn copy_from_linux_if_changed(state: &mut FromLinuxState) -> anyhow::Result<bool> {
    let read_result = read_from_linux_exact(&state.target.source_wine_path, &mut state.current);

    match read_result {
        Ok(()) => {
            if !state.source_was_available {
                info!("source became available: {}", state.target.source_host_path);
                state.source_was_available = true;
            }
            if state.source_size_mismatch {
                info!(
                    "source size matches expected size again: {}",
                    state.target.source_host_path
                );
                state.source_size_mismatch = false;
            }
        }
        Err(error) => {
            let message = format!("{error:#}");
            let is_size_mismatch = message.contains("expected");

            if is_size_mismatch {
                if !state.source_size_mismatch {
                    warn!(
                        "source size mismatch: {} ({message})",
                        state.target.source_host_path
                    );
                    state.source_size_mismatch = true;
                }
                return Ok(false);
            }

            if state.source_was_available {
                warn!(
                    "source became unavailable: {} ({message})",
                    state.target.source_host_path
                );
                state.source_was_available = false;
            } else {
                debug!(
                    "source still unavailable: {} ({message})",
                    state.target.source_host_path
                );
            }
            return Ok(false);
        }
    }

    if state.current == state.previous {
        return Ok(false);
    }

    unsafe {
        ptr::copy_nonoverlapping(
            state.current.as_ptr(),
            state.destination_mapping.view,
            state.current.len(),
        );
    }
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
    if !args.mappings.is_empty() {
        warn!(
            "--map is deprecated and will be removed in a future version; use --from-wine instead"
        );
    }
    if args.mappings.is_empty() && args.from_wine.is_empty() && args.from_linux.is_empty() {
        bail!("at least one mapping must be provided via --map, --from-wine, or --from-linux");
    }
    install_console_ctrl_handler()?;
    let lmu_lock = if args.lmu_lock {
        let lock = LmuSharedMemoryLock::initialize()?;
        info!("LMU compatibility lock initialized");
        Some(lock)
    } else {
        None
    };

    let from_wine_targets = build_from_wine_targets(&args);
    let mut from_linux_targets = build_from_linux_targets(&args);

    info!(
        "starting mirror loop for {} from-wine mapping(s) and {} from-linux mapping(s) with interval={}ms",
        from_wine_targets.len(),
        from_linux_targets.len(),
        args.interval_ms
    );

    for target in &from_wine_targets {
        info!(
            "from-wine {} -> {} (host {}, linux path {}) [{} bytes]",
            target.mapping_name,
            target.destination_wine_path.display(),
            target.destination_host_path,
            target.linux_path,
            target
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "auto".to_string())
        );
    }

    for target in &from_linux_targets {
        info!(
            "from-linux {} -> {} [{} bytes]",
            target.source_host_path,
            target.mapping_name,
            target
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "auto".to_string())
        );
    }

    let interval = Duration::from_millis(args.interval_ms);
    if let Some(event_name) = args
        .event
        .as_deref()
        .filter(|_| !from_wine_targets.is_empty())
    {
        wait_for_event_ready(event_name, interval);
        if shutdown_requested() {
            info!("shutdown requested before event became ready");
            return Ok(());
        }
    }
    resolve_from_linux_target_sizes(&mut from_linux_targets, interval);
    if shutdown_requested() {
        info!("shutdown requested before Linux sources became ready");
        return Ok(());
    }

    let mut from_wine_states = from_wine_targets
        .into_iter()
        .map(initialize_from_wine_state)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut from_linux_states = from_linux_targets
        .into_iter()
        .map(initialize_from_linux_state)
        .collect::<anyhow::Result<Vec<_>>>()?;

    let result = (|| -> anyhow::Result<()> {
        loop {
            if let Some(event_name) = args
                .event
                .as_deref()
                .filter(|_| !from_wine_states.is_empty())
            {
                if !wait_for_event_signal(event_name, interval)? {
                    break;
                }
            }

            let mut copied_count = 0usize;

            for state in &mut from_wine_states {
                if copy_from_wine_if_changed(state, lmu_lock.as_ref(), interval)? {
                    copied_count += 1;
                }
            }

            for state in &mut from_linux_states {
                if copy_from_linux_if_changed(state)? {
                    copied_count += 1;
                }
            }

            trace!("poll iteration complete; updated {copied_count} mapping(s)");

            if args.once || shutdown_requested() {
                break;
            }

            if args.event.is_none() || from_wine_states.is_empty() {
                thread::sleep(interval);
            }
        }

        Ok(())
    })();

    cleanup_from_wine_states(from_wine_states, args.keep_output_on_exit);
    drop(from_linux_states);

    result
}

#[cfg(test)]
mod tests {
    use super::{
        parse_from_linux, parse_from_wine, parse_mapping, unix_path_to_wine, validate_linux_path,
    };

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
    fn validates_linux_paths_with_subdirs() {
        assert_eq!(validate_linux_path("nested/name").unwrap(), "nested/name");
        assert!(validate_linux_path(r"nested\name").is_err());
        assert!(validate_linux_path("..").is_err());
        assert!(validate_linux_path("../name").is_err());
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

    #[test]
    fn from_wine_defaults_linux_path_from_namespace() {
        let mapping = parse_from_wine(r"Local\Telemetry").unwrap();
        assert_eq!(mapping.mapping_name, r"Local\Telemetry");
        assert_eq!(mapping.linux_path, "Local/Telemetry");
        assert_eq!(mapping.size, None);
    }

    #[test]
    fn from_linux_defaults_mapping_to_basename() {
        let mapping = parse_from_linux("foo/bar").unwrap();
        assert_eq!(mapping.linux_path, "foo/bar");
        assert_eq!(mapping.mapping_name, "bar");
        assert_eq!(mapping.size, None);
    }
}
