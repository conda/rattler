//! File locking for prefix directories
//!
//! Implementation of file locks adapted from cargo:
//! <https://github.com/rust-lang/cargo/blob/39c13e67a5962466cc7253d41bc1099bbcb224c3/src/cargo/util/flock.rs>
//!
//! Under MIT license.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use sys::{lock_exclusive, try_lock_exclusive, unlock};

const GUARD_PATH: &str = ".guard";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GuardState {
    /// Unknown
    Unknown,

    /// The prefix is currently being installed.
    Installing,

    /// The prefix has been successfully installed and is ready to be used.
    Ready,
}

#[derive(Debug)]
pub struct AsyncWriteGuard {
    file: Option<File>,
    state: GuardState,
}

impl AsyncWriteGuard {
    async fn new(mut file: File) -> io::Result<Self> {
        // Read the current state from the file (in a blocking task)
        let (state, file) =
            tokio::task::spawn_blocking(move || -> io::Result<(GuardState, File)> {
                let mut bytes = Vec::new();
                #[allow(clippy::verbose_file_reads)] // reading from an already-open locked file
                file.read_to_end(&mut bytes)?;
                let state = serde_json::from_slice(&bytes).unwrap_or(GuardState::Unknown);
                Ok((state, file))
            })
            .await
            .map_err(io::Error::other)??;

        Ok(Self {
            file: Some(file),
            state,
        })
    }

    /// Returns true if the prefix is in a usable state.
    pub fn is_ready(&self) -> bool {
        self.state == GuardState::Ready
    }

    /// Notify this instance that installation of the prefix has started.
    pub async fn begin(&mut self) -> io::Result<()> {
        if self.state != GuardState::Installing {
            let mut file = self
                .file
                .take()
                .ok_or_else(|| io::Error::other("File already consumed"))?;

            // Write the installing state (in a blocking task)
            file = tokio::task::spawn_blocking(move || -> io::Result<File> {
                file.seek(SeekFrom::Start(0))?;
                let bytes = serde_json::to_vec(&GuardState::Installing)?;
                file.write_all(&bytes)?;
                file.set_len(bytes.len() as u64)?;
                file.flush()?;
                Ok(file)
            })
            .await
            .map_err(io::Error::other)??;

            self.file = Some(file);
            self.state = GuardState::Installing;
        }
        Ok(())
    }

    /// Finishes writing to the guard and releases the lock.
    pub async fn finish(mut self) -> io::Result<()> {
        let status = self.state;

        if status == GuardState::Installing {
            let Some(mut file) = self.file.take() else {
                return Ok(());
            };
            // Write the ready state (in a blocking task)
            tokio::task::spawn_blocking(move || -> io::Result<()> {
                file.seek(SeekFrom::Start(0))?;
                let bytes = serde_json::to_vec(&GuardState::Ready)?;
                file.write_all(&bytes)?;
                file.set_len(bytes.len() as u64)?;
                file.flush()?;
                Ok(())
            })
            .await
            .map_err(io::Error::other)??;
        }
        Ok(())
    }
}

impl Drop for AsyncWriteGuard {
    fn drop(&mut self) {
        if let Some(f) = self.file.take() {
            let _ = unlock(&f);
        }
    }
}

pub struct AsyncPrefixGuard {
    guard_path: PathBuf,
}

impl AsyncPrefixGuard {
    /// Constructs a new guard for the given prefix but does not perform any
    /// locking operations yet.
    pub async fn new(prefix: &Path) -> io::Result<Self> {
        let guard_path = prefix.join(GUARD_PATH);

        // Ensure that the directory exists
        fs_err::tokio::create_dir_all(guard_path.parent().unwrap()).await?;

        Ok(Self { guard_path })
    }

    /// Locks the guard for writing and returns a write guard which can be used
    /// to unlock it.
    pub async fn write(self) -> io::Result<AsyncWriteGuard> {
        let guard_path = self.guard_path.clone();

        // Open the file and acquire the lock (in a blocking task)
        let file = tokio::task::spawn_blocking(move || -> io::Result<File> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&guard_path)?;

            lock_exclusive(&file)?;

            Ok(file)
        })
        .await
        .map_err(io::Error::other)??;

        AsyncWriteGuard::new(file).await
    }
}

#[cfg(unix)]
mod sys {
    use std::fs::File;
    use std::io::{Error, Result};
    use std::os::unix::io::AsRawFd;

    pub fn lock_exclusive(file: &File) -> Result<()> {
        flock(file, libc::LOCK_EX)
    }

    pub fn try_lock_exclusive(file: &File) -> Result<()> {
        flock(file, libc::LOCK_EX | libc::LOCK_NB)
    }

    pub fn unlock(file: &File) -> Result<()> {
        flock(file, libc::LOCK_UN)
    }

    #[allow(dead_code)]
    pub fn error_contended(err: &Error) -> bool {
        err.raw_os_error() == Some(libc::EWOULDBLOCK)
    }

    #[allow(dead_code, unreachable_patterns)]
    pub fn error_unsupported(err: &Error) -> bool {
        matches!(
            err.raw_os_error(),
            Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::ENOSYS)
        )
    }

    #[cfg(not(target_os = "solaris"))]
    fn flock(file: &File, flag: libc::c_int) -> Result<()> {
        let ret = unsafe { libc::flock(file.as_raw_fd(), flag) };
        if ret < 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "solaris")]
    fn flock(file: &File, flag: libc::c_int) -> Result<()> {
        // Solaris lacks flock(), so try to emulate using fcntl()
        let mut flock = libc::flock {
            l_type: 0,
            l_whence: 0,
            l_start: 0,
            l_len: 0,
            l_sysid: 0,
            l_pid: 0,
            l_pad: [0, 0, 0, 0],
        };
        flock.l_type = if flag & libc::LOCK_UN != 0 {
            libc::F_UNLCK
        } else if flag & libc::LOCK_EX != 0 {
            libc::F_WRLCK
        } else if flag & libc::LOCK_SH != 0 {
            libc::F_RDLCK
        } else {
            panic!("unexpected flock() operation")
        };

        let mut cmd = libc::F_SETLKW;
        if (flag & libc::LOCK_NB) != 0 {
            cmd = libc::F_SETLK;
        }

        let ret = unsafe { libc::fcntl(file.as_raw_fd(), cmd, &flock) };

        if ret < 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::fs::File;
    use std::io::{Error, Result};
    use std::mem;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Foundation::{ERROR_INVALID_FUNCTION, ERROR_LOCK_VIOLATION};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };

    pub fn lock_exclusive(file: &File) -> Result<()> {
        lock_file(file, LOCKFILE_EXCLUSIVE_LOCK)
    }

    pub fn try_lock_exclusive(file: &File) -> Result<()> {
        lock_file(file, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)
    }

    #[allow(dead_code)]
    pub fn error_contended(err: &Error) -> bool {
        err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
    }

    #[allow(dead_code)]
    pub fn error_unsupported(err: &Error) -> bool {
        err.raw_os_error() == Some(ERROR_INVALID_FUNCTION as i32)
    }

    pub fn unlock(file: &File) -> Result<()> {
        unsafe {
            let ret = UnlockFile(file.as_raw_handle() as HANDLE, 0, 0, !0, !0);
            if ret == 0 {
                Err(Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }

    fn lock_file(file: &File, flags: u32) -> Result<()> {
        unsafe {
            let mut overlapped = mem::zeroed();
            let ret = LockFileEx(
                file.as_raw_handle() as HANDLE,
                flags,
                0,
                !0,
                !0,
                &mut overlapped,
            );
            if ret == 0 {
                Err(Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}
