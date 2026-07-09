//! ProjFS (Windows Projected File System) transport adapter.
//!
//! Maps ProjFS callbacks to `VfsOps` trait methods. ProjFS is demand-driven:
//! files start as placeholders (metadata only) and are hydrated (content
//! materialized) on first read. Once hydrated, reads go directly to disk.
//!
//! This adapter is Windows-only and requires Windows 10 version 1809+.

// The ProjFS callbacks are `unsafe extern "system" fn`s whose bodies are almost
// entirely raw-pointer FFI against the Win32 API. Under Rust 2024's
// `unsafe_op_in_unsafe_fn` every deref/call would otherwise need its own
// `unsafe {}` wrapper; the whole module is inherently unsafe FFI, so allow it
// module-wide rather than peppering the callbacks with blocks.
#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::Storage::ProjectedFileSystem::*;
use windows::core::{GUID, HRESULT, PCWSTR};

use crate::vfs_ops::{FileAttr, FileKind, VfsOps};

/// Cached state for an in-progress directory enumeration.
///
/// ProjFS calls `start_dir_enum_cb` once, then `get_dir_enum_cb` repeatedly
/// to stream entries until the buffer is full; `end_dir_enum_cb` releases
/// the state. The snapshot + cursor approach keeps the enumeration stable
/// under concurrent reads.
struct EnumState {
    entries: Vec<(String, FileAttr)>,
    index: usize,
}

/// Wrap a ProjFS callback body in `catch_unwind`.
///
/// Any Rust panic unwinding across the `unsafe extern "system"` FFI boundary
/// into Windows is undefined behaviour. We catch panics, log them, and return
/// `ERROR_INTERNAL_ERROR` so ProjFS enters a consistent failed state instead
/// of a UB state. This preserves the "fail the operation, keep the mount
/// alive" contract that the rest of the adapter relies on.
fn projfs_cb<F>(name: &'static str, f: F) -> HRESULT
where
    F: FnOnce() -> HRESULT,
{
    // AssertUnwindSafe: ProjFS callbacks don't share mutable state via
    // captured references — they access it through the adapter pointer and
    // per-enumeration map, both of which are internally synchronized.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match result {
        Ok(hr) => hr,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic>".to_string());
            tracing::error!("ProjFS callback `{}` panicked: {}", name, msg);
            HRESULT::from(windows::Win32::Foundation::ERROR_INTERNAL_ERROR)
        }
    }
}

/// ProjFS transport adapter, generic over any `VfsOps` implementation.
///
/// ProjFS is always writable: it writes hydrated content directly to the
/// virtualization root and tracks deletions via tombstones. Read-only mode
/// cannot be enforced because ProjFS has no pre-creation notification —
/// new files can always be created regardless of notification callbacks.
pub struct ProjFsAdapter<T: VfsOps> {
    vfs: Arc<T>,
    /// Cache of path → inode mappings for fast lookup.
    path_cache: Mutex<HashMap<PathBuf, u64>>,
    /// Per-adapter directory enumeration state, keyed by the ProjFS-assigned
    /// enumeration GUID. Previously a process-global `LazyLock<Mutex<...>>`,
    /// which tied state to the process (two side-by-side mounts shared the
    /// map and a failed Drop leaked globally). Moving it onto the adapter
    /// ties lifetime to the mount — clean up is automatic.
    enum_sessions: Mutex<HashMap<GUID, EnumState>>,
    /// Absolute path to the virtualization root directory.
    root: PathBuf,
    /// Kept alive for the duration of the virtualization instance.
    /// ProjFS may reference this after PrjStartVirtualizing returns.
    _notification_root: Vec<u16>,
}

impl<T: VfsOps> ProjFsAdapter<T> {
    pub fn new(vfs: T) -> Self {
        let mut path_cache = HashMap::new();
        // Root inode is always 1
        path_cache.insert(PathBuf::new(), 1);
        // Empty wide string for NotificationRoot (means "root of virtualization instance")
        let notification_root: Vec<u16> = vec![0u16];
        Self {
            vfs: Arc::new(vfs),
            path_cache: Mutex::new(path_cache),
            enum_sessions: Mutex::new(HashMap::new()),
            root: PathBuf::new(), // set in start()
            _notification_root: notification_root,
        }
    }

    /// Resolve a path to an inode, walking the VFS tree component by component.
    /// Caches intermediate results for performance.
    fn resolve_path(&self, path: &Path) -> Result<u64, i32> {
        // Check cache first
        {
            let cache = self.path_cache.lock().unwrap();
            if let Some(&ino) = cache.get(path) {
                return Ok(ino);
            }
        }

        // Walk from root
        let mut current_ino = 1u64;
        let mut current_path = PathBuf::new();
        let mut new_entries = Vec::new();

        for component in path.components() {
            let name = component.as_os_str();
            current_path.push(name);

            // Check cache for intermediate path
            {
                let cache = self.path_cache.lock().unwrap();
                if let Some(&cached_ino) = cache.get(&current_path) {
                    current_ino = cached_ino;
                    continue;
                }
            }

            let attr = self.vfs.lookup(current_ino, name)?;
            current_ino = attr.ino;
            new_entries.push((current_path.clone(), current_ino));
        }

        // Batch insert cache entries
        if !new_entries.is_empty() {
            let mut cache = self.path_cache.lock().unwrap();
            for (path, ino) in new_entries {
                cache.insert(path, ino);
            }
        }

        Ok(current_ino)
    }

    /// Resolve a path's parent inode and filename.
    fn resolve_parent_and_name<'a>(
        &self,
        path: &'a Path,
    ) -> Result<(u64, &'a std::ffi::OsStr), i32> {
        let parent = path.parent().unwrap_or(Path::new(""));
        let name = path.file_name().ok_or(libc::ENOENT)?;
        let parent_ino = self.resolve_path(parent)?;
        Ok((parent_ino, name))
    }

    /// Start the ProjFS virtualization instance.
    ///
    /// The `root` directory must already exist and will be marked as the
    /// virtualization root. Returns a handle that stops virtualization on drop.
    pub fn start(mut self, root: &Path) -> anyhow::Result<ProjFsHandle> {
        use std::os::windows::ffi::OsStrExt;

        self.root = root.to_path_buf();

        // Mark the directory as a virtualization root
        let root_wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();

        // Mark the directory as a virtualization root. Ignore ERROR_ALREADY_EXISTS
        // which means it was already marked (e.g. from a previous mount session).
        unsafe {
            let result = PrjMarkDirectoryAsPlaceholder(
                PCWSTR(root_wide.as_ptr()),
                PCWSTR::null(),
                None,
                &PRJ_PLACEHOLDER_ID,
            );
            if let Err(ref e) = result {
                if e.code() != HRESULT::from(windows::Win32::Foundation::ERROR_ALREADY_EXISTS) {
                    result?;
                }
            }
        }

        let adapter = Arc::new(self);

        // Set up callbacks
        let callbacks = PRJ_CALLBACKS {
            StartDirectoryEnumerationCallback: Some(start_dir_enum_cb::<T>),
            EndDirectoryEnumerationCallback: Some(end_dir_enum_cb::<T>),
            GetDirectoryEnumerationCallback: Some(get_dir_enum_cb::<T>),
            GetPlaceholderInfoCallback: Some(get_placeholder_info_cb::<T>),
            GetFileDataCallback: Some(get_file_data_cb::<T>),
            // QueryFileNameCallback lets ProjFS ask whether a path exists in
            // our backing store.  EdenFS registers this; ProjFS uses it to
            // decide whether a placeholder is "projected" vs user-created.
            QueryFileNameCallback: Some(query_file_name_cb::<T>),
            // Notification callback for rename support and state tracking.
            NotificationCallback: Some(notification_cb::<T>),
            ..Default::default()
        };

        let context = Arc::into_raw(adapter.clone()) as *const std::ffi::c_void;

        // Match EdenFS: register for rename and post-mutation events so
        // ProjFS can track placeholder state transitions accurately.
        let notification_bits = PRJ_NOTIFY_PRE_RENAME
            | PRJ_NOTIFY_FILE_RENAMED
            | PRJ_NOTIFY_NEW_FILE_CREATED
            | PRJ_NOTIFY_FILE_HANDLE_CLOSED_FILE_MODIFIED
            | PRJ_NOTIFY_FILE_HANDLE_CLOSED_FILE_DELETED;
        let mut notification_mapping = PRJ_NOTIFICATION_MAPPING {
            NotificationBitMask: notification_bits,
            NotificationRoot: PCWSTR(adapter._notification_root.as_ptr()),
        };
        let options = PRJ_STARTVIRTUALIZING_OPTIONS {
            NotificationMappings: &mut notification_mapping as *mut _,
            NotificationMappingsCount: 1,
            // Use enough pool threads so that GetFileDataCallback can be
            // dispatched while a notification callback (PRE_RENAME) is blocked
            // waiting for force_hydrate to finish reading placeholder files.
            // With the default (0 → system-chosen, often 1), the single pool
            // thread runs the notification callback and no thread is left to
            // service the hydration reads, causing a deadlock.
            PoolThreadCount: 4,
            ConcurrentThreadCount: 4,
            ..Default::default()
        };

        let virt_context = unsafe {
            PrjStartVirtualizing(
                PCWSTR(root_wide.as_ptr()),
                &callbacks,
                Some(context),
                Some(&options as *const _),
            )?
        };

        Ok(ProjFsHandle {
            context: virt_context,
            _adapter: adapter,
            stopped: false,
        })
    }
}

/// Handle to a running ProjFS virtualization instance. Stops on drop.
pub struct ProjFsHandle {
    context: PRJ_NAMESPACE_VIRTUALIZATION_CONTEXT,
    _adapter: Arc<dyn std::any::Any + Send + Sync>,
    /// Whether `PrjStopVirtualizing` has already been called. Set by
    /// `unmount()`; checked by `Drop` to avoid double-stopping.
    stopped: bool,
}

impl ProjFsHandle {
    /// Explicitly stop ProjFS virtualization.
    ///
    /// Prefer this over Drop when error handling matters. `PrjStopVirtualizing`
    /// itself returns no error, so this method always returns `Ok(())`; it
    /// exists for parity with `MountHandle::unmount` and to give callers a
    /// single explicit teardown point.
    pub fn unmount(mut self) -> anyhow::Result<()> {
        if !self.stopped {
            unsafe {
                PrjStopVirtualizing(self.context);
            }
            self.stopped = true;
        }
        Ok(())
    }
}

impl Drop for ProjFsHandle {
    fn drop(&mut self) {
        if !self.stopped {
            unsafe {
                PrjStopVirtualizing(self.context);
            }
        }
    }
}

// Placeholder GUID for our virtualization root
const PRJ_PLACEHOLDER_ID: GUID = GUID::from_u128(0x72617474_6c65_7266_735f_706c61636568);

// ---------------------------------------------------------------------------
// ProjFS callback implementations
// ---------------------------------------------------------------------------

/// Convert a PCWSTR to a PathBuf
fn pcwstr_to_path(s: PCWSTR) -> PathBuf {
    unsafe {
        let len = (0..).take_while(|&i| *s.0.add(i) != 0).count();
        let slice = std::slice::from_raw_parts(s.0, len);
        PathBuf::from(OsString::from_wide(slice))
    }
}

/// Get the adapter from the callback data's instance context
unsafe fn get_adapter<T: VfsOps>(data: *const PRJ_CALLBACK_DATA) -> &'static ProjFsAdapter<T> {
    let context = (*data).InstanceContext as *const ProjFsAdapter<T>;
    &*context
}

/// Query whether a path exists in the backing store.
///
/// ProjFS calls this to determine if a path is projected (exists in the
/// provider) or local-only.
unsafe extern "system" fn query_file_name_cb<T: VfsOps>(data: *const PRJ_CALLBACK_DATA) -> HRESULT {
    projfs_cb("query_file_name", || {
        let adapter = get_adapter::<T>(data);
        let path = pcwstr_to_path((*data).FilePathName);
        match adapter.resolve_path(&path) {
            Ok(_) => HRESULT(0),
            Err(_) => HRESULT::from(ERROR_FILE_NOT_FOUND),
        }
    })
}

/// Convert FileAttr to PRJ_FILE_BASIC_INFO
fn attr_to_basic_info(attr: &FileAttr) -> PRJ_FILE_BASIC_INFO {
    fn system_time_to_filetime(t: std::time::SystemTime) -> i64 {
        // Windows FILETIME: 100ns intervals since 1601-01-01
        // Unix epoch: 1970-01-01 = 11644473600 seconds after 1601-01-01
        const EPOCH_DIFF: i64 = 11_644_473_600;
        match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64 + EPOCH_DIFF) * 10_000_000 + d.subsec_nanos() as i64 / 100,
            Err(_) => 0,
        }
    }

    let ft = system_time_to_filetime(attr.mtime);
    PRJ_FILE_BASIC_INFO {
        IsDirectory: attr.kind == FileKind::Directory,
        FileSize: attr.size as i64,
        CreationTime: ft,
        LastAccessTime: ft,
        LastWriteTime: ft,
        ChangeTime: ft,
        FileAttributes: if attr.kind == FileKind::Directory {
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY.0
        } else {
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL.0
        },
    }
}

unsafe extern "system" fn start_dir_enum_cb<T: VfsOps>(
    data: *const PRJ_CALLBACK_DATA,
    enumeration_id: *const GUID,
) -> HRESULT {
    projfs_cb("start_dir_enum", || {
        let adapter = get_adapter::<T>(data);
        let path = pcwstr_to_path((*data).FilePathName);

        let ino = match adapter.resolve_path(&path) {
            Ok(ino) => ino,
            Err(_) => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        let dir_entries = match adapter.vfs.readdir(ino, 0) {
            Ok(entries) => entries,
            Err(_) => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        // Resolve attrs for each entry
        let mut entries = Vec::new();
        for de in dir_entries {
            if de.name == "." || de.name == ".." {
                continue;
            }
            if let Ok(attr) = adapter.vfs.getattr(de.ino) {
                let name = de.name.to_string_lossy().to_string();
                entries.push((name, attr));
            }
        }

        // Sort by name (ProjFS requires sorted enumeration results)
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        adapter
            .enum_sessions
            .lock()
            .unwrap()
            .insert(*enumeration_id, EnumState { entries, index: 0 });
        HRESULT(0)
    })
}

unsafe extern "system" fn end_dir_enum_cb<T: VfsOps>(
    data: *const PRJ_CALLBACK_DATA,
    enumeration_id: *const GUID,
) -> HRESULT {
    projfs_cb("end_dir_enum", || {
        let adapter = get_adapter::<T>(data);
        adapter
            .enum_sessions
            .lock()
            .unwrap()
            .remove(&*enumeration_id);
        HRESULT(0)
    })
}

unsafe extern "system" fn get_dir_enum_cb<T: VfsOps>(
    data: *const PRJ_CALLBACK_DATA,
    enumeration_id: *const GUID,
    search_expression: PCWSTR,
    dir_entry_buffer_handle: PRJ_DIR_ENTRY_BUFFER_HANDLE,
) -> HRESULT {
    projfs_cb("get_dir_enum", || {
        use std::os::windows::ffi::OsStrExt;

        let adapter = get_adapter::<T>(data);
        let mut sessions = adapter.enum_sessions.lock().unwrap();
        let state = match sessions.get_mut(&*enumeration_id) {
            Some(s) => s,
            None => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        // If this is a restart, reset the index
        if ((*data).Flags.0 & PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN.0) != 0 {
            state.index = 0;
        }

        while state.index < state.entries.len() {
            let (ref name, ref attr) = state.entries[state.index];

            // Apply search expression filter if provided
            if !search_expression.is_null() {
                let name_wide: Vec<u16> = std::ffi::OsStr::new(name)
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                if PrjFileNameMatch(PCWSTR(name_wide.as_ptr()), search_expression) == false {
                    state.index += 1;
                    continue;
                }
            }

            let basic_info = attr_to_basic_info(attr);

            let name_wide: Vec<u16> = std::ffi::OsStr::new(name)
                .encode_wide()
                .chain(Some(0))
                .collect();
            let result = PrjFillDirEntryBuffer(
                PCWSTR(name_wide.as_ptr()),
                Some(&basic_info),
                dir_entry_buffer_handle,
            );

            if result.is_err() {
                // Buffer full — ProjFS will call us again
                break;
            }

            state.index += 1;
        }

        HRESULT(0)
    })
}

unsafe extern "system" fn get_placeholder_info_cb<T: VfsOps>(
    data: *const PRJ_CALLBACK_DATA,
) -> HRESULT {
    projfs_cb("get_placeholder_info", || {
        let adapter = get_adapter::<T>(data);
        let path = pcwstr_to_path((*data).FilePathName);

        let (parent_ino, name) = match adapter.resolve_parent_and_name(&path) {
            Ok(v) => v,
            Err(_) => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        let attr = match adapter.vfs.lookup(parent_ino, name) {
            Ok(a) => a,
            Err(_) => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        let basic_info = attr_to_basic_info(&attr);
        let placeholder_info = PRJ_PLACEHOLDER_INFO {
            FileBasicInfo: basic_info,
            ..Default::default()
        };

        let result = PrjWritePlaceholderInfo(
            (*data).NamespaceVirtualizationContext,
            (*data).FilePathName,
            &placeholder_info,
            std::mem::size_of::<PRJ_PLACEHOLDER_INFO>() as u32,
        );

        match result {
            Ok(()) => HRESULT(0),
            Err(e) => e.code(),
        }
    })
}

unsafe extern "system" fn get_file_data_cb<T: VfsOps>(
    data: *const PRJ_CALLBACK_DATA,
    byte_offset: u64,
    length: u32,
) -> HRESULT {
    projfs_cb("get_file_data", || {
        let adapter = get_adapter::<T>(data);
        let path = pcwstr_to_path((*data).FilePathName);

        let ino = match adapter.resolve_path(&path) {
            Ok(ino) => ino,
            Err(_) => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        // Read the content from VFS
        let content = match adapter.vfs.read(ino, byte_offset, length) {
            Ok(data) => data,
            Err(_) => return HRESULT::from(ERROR_FILE_NOT_FOUND),
        };

        // Allocate a ProjFS-aligned write buffer and copy content into it
        let buffer =
            PrjAllocateAlignedBuffer((*data).NamespaceVirtualizationContext, content.len());
        if buffer.is_null() {
            return HRESULT::from(windows::Win32::Foundation::ERROR_OUTOFMEMORY);
        }

        std::ptr::copy_nonoverlapping(content.as_ptr(), buffer as *mut u8, content.len());

        let result = PrjWriteFileData(
            (*data).NamespaceVirtualizationContext,
            &(*data).DataStreamId,
            buffer,
            byte_offset,
            content.len() as u32,
        );

        PrjFreeAlignedBuffer(buffer);

        match result {
            Ok(()) => HRESULT(0),
            Err(e) => e.code(),
        }
    })
}

/// Recursively read all files under `dir` to force ProjFS to hydrate
/// placeholders to on-disk content, and create+delete a temp file in
/// each directory to mark it as "dirty" (modified). After this, ProjFS
/// allows the directory to be renamed.
fn force_hydrate(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("force_hydrate: read_dir({dir:?}) failed: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            force_hydrate(&path);
        } else {
            // Reading triggers ProjFS GetFileDataCallback on a pool thread,
            // which writes content to disk (placeholder → hydrated).
            if let Err(e) = std::fs::read(&path) {
                tracing::warn!("force_hydrate: read({path:?}) failed: {e}");
            }
        }
    }
    // Create and delete a temp file to mark the directory as "dirty"
    // (modified). ProjFS may reject renames of clean placeholder
    // directories even after their children are hydrated.
    let marker = dir.join(".rattler_vfs_hydrate_marker");
    if std::fs::write(&marker, b"").is_ok() {
        let _ = std::fs::remove_file(&marker);
    }
}

/// Notification callback for rename support.
///
/// Force-hydrates placeholder directories before rename so ProjFS allows
/// the operation on the now-full items. Hydration runs on a separate
/// thread to avoid re-entrancy deadlocks with ProjFS callbacks.
unsafe extern "system" fn notification_cb<T: VfsOps>(
    data: *const PRJ_CALLBACK_DATA,
    is_directory: bool,
    notification: PRJ_NOTIFICATION,
    _dest_filename: PCWSTR,
    _parameters: *mut PRJ_NOTIFICATION_PARAMETERS,
) -> HRESULT {
    projfs_cb("notification", || {
        let n = notification.0;

        // Force-hydrate directory content before rename so ProjFS allows the
        // operation on the now-full items. Hydration runs on a separate thread
        // to avoid re-entrancy deadlocks: reading files from within a ProjFS
        // callback would dispatch GetFileDataCallback on the same pool,
        // potentially deadlocking. PoolThreadCount is set to 4 in start() to
        // ensure enough threads.
        if n == PRJ_NOTIFICATION_PRE_RENAME.0 {
            let adapter = get_adapter::<T>(data);
            let rel_path = pcwstr_to_path((*data).FilePathName);
            let full_path = adapter.root.join(&rel_path);
            if is_directory {
                tracing::info!("PRE_RENAME dir: {full_path:?}, spawning hydration thread");
                let handle = std::thread::spawn(move || {
                    force_hydrate(&full_path);
                });
                if let Err(e) = handle.join() {
                    tracing::error!("hydration thread panicked: {e:?}");
                }
                tracing::info!("PRE_RENAME hydration complete, allowing rename");
            } else {
                tracing::debug!("PRE_RENAME file: {full_path:?}");
            }
        }

        HRESULT(0)
    })
}
