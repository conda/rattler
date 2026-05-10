//! Windows security primitives that back the platform-portable
//! helpers in [`crate`]. Provides the analogues of the Unix
//! `openat`+`mkdir 0o700` atomic creation and `geteuid()`-based
//! ownership check.
//!
//! Four building blocks:
//!
//! * [`owner_sid`] -- process-cached current-user SID, drawn once
//!   from the access token via `OpenProcessToken` +
//!   `GetTokenInformation(TokenUser)`.
//! * [`create_owner_only_subdir`] -- fd-anchored atomic mkdir
//!   relative to a parent `Dir`'s handle, with a `PROTECTED` DACL
//!   granting only the current user. Implemented through
//!   `NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory` set to
//!   the parent and an explicit `SECURITY_DESCRIPTOR` -- there is
//!   no Win32 equivalent, but `NtCreateFile` is stable.
//! * [`rename_via_handle`] -- fd-anchored rename using
//!   `SetFileInformationByHandle(FileRenameInfoEx)` with
//!   `RootDirectory` set to the destination directory's handle.
//!   No path resolution happens; the destination name is single-
//!   component (enforced upstream).
//! * [`verify_owner`] -- defence-in-depth check that the kernel
//!   honoured our requested owner SID on the freshly-created
//!   directory. Useful on filesystems that don't support
//!   security descriptors (FAT) and as a backstop against future
//!   kernel/cap-std changes.
//!
//! Why this design closes the remaining TOCTOU windows:
//! `NtCreateFile` with `RootDirectory` is the only way to do a
//! true fd-anchored `mkdirat`/`openat` on Windows -- every Win32
//! `CreateFileW` / `CreateDirectoryW` path goes through fresh
//! name resolution. By passing the parent handle in
//! `OBJECT_ATTRIBUTES.RootDirectory` and supplying the security
//! descriptor in the same call, the create is atomic with respect
//! to both placement (no intermediate-component race) and
//! permissions (no inherit-then-harden window).

use std::ffi::OsStr;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::sync::OnceLock;

use cap_std::fs::Dir;
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE,
    FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    LocalFree, RtlNtStatusToDosError, ERROR_CANT_ACCESS_FILE, ERROR_INSUFFICIENT_BUFFER,
    ERROR_STOPPED_ON_SYMLINK, GENERIC_READ, HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, STATUS_SUCCESS,
    UNICODE_STRING,
};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    AddAccessAllowedAceEx, CopySid, EqualSid, GetLengthSid, GetTokenInformation, InitializeAcl,
    IsValidSid, TokenUser, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, CONTAINER_INHERIT_ACE,
    OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_DESCRIPTOR, SE_DACL_PRESENT, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
#[cfg(test)]
use windows_sys::Win32::Security::{GetSecurityDescriptorControl, DACL_SECURITY_INFORMATION};
use windows_sys::Win32::Storage::FileSystem::{
    FileRenameInfoEx, SetFileInformationByHandle, DELETE, FILE_GENERIC_WRITE, FILE_RENAME_INFO,
    FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Access mask used when opening the payload file via
/// [`create_owner_only_file`]. `FILE_GENERIC_WRITE` lets us write
/// the bytes; `DELETE` is required so the fd-anchored
/// [`rename_via_handle`] can publish the file without reopening.
const PAYLOAD_ACCESS_MASK: u32 = FILE_GENERIC_WRITE | DELETE;

/// `FILE_ALL_ACCESS` = `STANDARD_RIGHTS_ALL | SYNCHRONIZE | 0x1FF`.
/// Hardcoded to avoid pulling in another windows-sys constant
/// just for the ACE rights mask.
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// `SECURITY_DESCRIPTOR_REVISION = 1`. Hardcoded to avoid pulling
/// in the `Win32_System_SystemServices` feature for a single
/// constant whose value has been stable since Windows NT 3.1.
const SECURITY_DESCRIPTOR_REVISION_1: u8 = 1;

/// `FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS`.
/// POSIX semantics make the rename atomic with respect to other
/// handles on the destination (matches what `renameat` does on
/// Unix). Hardcoded to avoid the `Win32_System_WindowsProgramming`
/// feature.
const FILE_RENAME_FLAGS: u32 = 0x1 | 0x2;

/// Process-cached owner SID. Returns a slice valid for the
/// remaining process lifetime.
pub(crate) fn owner_sid() -> io::Result<&'static [u8]> {
    static SID: OnceLock<Vec<u8>> = OnceLock::new();
    if let Some(sid) = SID.get() {
        return Ok(sid.as_slice());
    }
    let sid = compute_owner_sid()?;
    // If another thread won the race the freshly-computed `sid`
    // is dropped and we return the winner's. Both SIDs must be
    // equal for the current process, so this is harmless.
    Ok(SID.get_or_init(|| sid).as_slice())
}

fn compute_owner_sid() -> io::Result<Vec<u8>> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that does
    // not need to be closed.
    let proc_handle = unsafe { GetCurrentProcess() };

    let mut raw_token: HANDLE = ptr::null_mut();
    // SAFETY: proc_handle is the current-process pseudo-handle;
    // raw_token is a valid out-pointer.
    let ok = unsafe { OpenProcessToken(proc_handle, TOKEN_QUERY, &mut raw_token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned a real handle we now own.
    let token = unsafe { OwnedHandle::from_raw_handle(raw_token.cast()) };
    let token_handle = token.as_raw_handle() as HANDLE;

    // Size-discovery probe: a zero-length buffer is documented to
    // fail with ERROR_INSUFFICIENT_BUFFER and write the required
    // length to `needed`.
    let mut needed: u32 = 0;
    // SAFETY: token_handle is a valid token handle from above;
    // null+0 is the documented size-discovery form; `needed` is a
    // valid out-pointer.
    let probe =
        unsafe { GetTokenInformation(token_handle, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if probe != 0 {
        return Err(io::Error::other(
            "GetTokenInformation unexpectedly succeeded with zero buffer",
        ));
    }
    let probe_err = io::Error::last_os_error();
    if probe_err.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(probe_err);
    }

    let mut buf = vec![0u8; needed as usize];
    // SAFETY: buf is `needed` bytes long; `needed` was set by the
    // probe above.
    let ok = unsafe {
        GetTokenInformation(
            token_handle,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: GetTokenInformation succeeded, so the start of buf
    // holds a valid TOKEN_USER.
    let token_user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    let sid = token_user.User.Sid;

    // SAFETY: sid was provided by the kernel inside `buf`.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(io::Error::other("token user SID is invalid"));
    }
    // SAFETY: sid was just validated.
    let sid_len = unsafe { GetLengthSid(sid) } as usize;
    let mut out = vec![0u8; sid_len];
    // SAFETY: out is sid_len bytes long; CopySid writes at most
    // that many bytes into a valid SID-shaped buffer.
    let ok = unsafe { CopySid(sid_len as u32, out.as_mut_ptr().cast(), sid) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(out)
}

/// fd-anchored atomic mkdir. Creates a directory named `name`
/// inside `parent`'s handle with a `PROTECTED` DACL whose ACE is
/// inheritable, so any file created inside it later (via
/// [`create_owner_only_file`]) starts with the same locked-down
/// access. The kernel applies the security descriptor as part of
/// the create call -- there is no inherit-then-harden window.
///
/// `FILE_CREATE` is exclusive: if anything exists at `name`
/// (file, dir, symlink, junction), the call fails.
/// `FILE_OPEN_REPARSE_POINT` is set defensively so that any
/// reparse point that does appear there is opened as the link
/// itself rather than followed -- combined with `FILE_CREATE` it
/// means we either succeed atomically or return an error; we
/// never silently traverse an attacker reparse point.
pub(crate) fn create_owner_only_subdir(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    let handle = create_owner_only_object(
        parent,
        name,
        GENERIC_READ | DELETE,
        FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
    )?;
    // SAFETY: NtCreateFile returned a valid HANDLE we now own.
    let std_file = unsafe { std::fs::File::from_raw_handle(handle.cast()) };
    Ok(Dir::from_std_file(std_file))
}

/// fd-anchored atomic file create. Same model as
/// [`create_owner_only_subdir`] but for a regular file: the file
/// is created inside `parent`'s handle with `FILE_CREATE` (no
/// overwrite) and a `PROTECTED` DACL granting only the current
/// user. The access mask is [`PAYLOAD_ACCESS_MASK`] so the
/// caller can write bytes and rename via [`rename_via_handle`]
/// without reopening.
///
/// Returns a [`std::fs::File`] rather than a `cap_std::fs::File`
/// because the caller (`atomic_write_in`) only needs `Write` /
/// `sync_all` / `AsRawHandle` on it, and going through cap-std
/// would re-introduce a path-based open.
pub(crate) fn create_owner_only_file(parent: &Dir, name: &OsStr) -> io::Result<std::fs::File> {
    let handle = create_owner_only_object(
        parent,
        name,
        PAYLOAD_ACCESS_MASK,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        // No inheritance flags: files have no children, so the
        // ACE flags are 0. The DACL is still `PROTECTED`, which
        // is what locks down the file regardless of what the
        // (post-rename) parent's ambient ACL looks like.
        0,
    )?;
    // SAFETY: NtCreateFile returned a valid HANDLE we now own.
    Ok(unsafe { std::fs::File::from_raw_handle(handle.cast()) })
}

/// Shared `NtCreateFile`-with-`FILE_CREATE` body for the
/// directory and file helpers above. Builds a `PROTECTED`
/// owner-only `SECURITY_DESCRIPTOR` whose single allow-ACE
/// carries `ace_flags` (set to inheritance bits for directories,
/// `0` for files) and applies it at create time so the new
/// object's effective DACL is exactly that ACE.
fn create_owner_only_object(
    parent: &Dir,
    name: &OsStr,
    desired_access: u32,
    create_options: u32,
    ace_flags: u32,
) -> io::Result<HANDLE> {
    let parent_handle = parent.as_raw_handle() as HANDLE;
    let sid = owner_sid()?;

    let mut acl = build_owner_only_acl(sid, ace_flags)?;
    let acl_ptr = acl.as_mut_ptr().cast::<ACL>();

    // Hand-construct the SECURITY_DESCRIPTOR. `SE_DACL_PRESENT`
    // says the DACL pointer is meaningful; `SE_DACL_PROTECTED`
    // blocks the default merge-with-parent-inheritable-ACEs that
    // NT would otherwise do at create time, so the new object's
    // effective DACL is exactly our one allow-ACE.
    let sd = SECURITY_DESCRIPTOR {
        Revision: SECURITY_DESCRIPTOR_REVISION_1,
        Sbz1: 0,
        Control: SE_DACL_PRESENT | SE_DACL_PROTECTED,
        Owner: sid.as_ptr() as PSID,
        Group: ptr::null_mut(),
        Sacl: ptr::null_mut(),
        Dacl: acl_ptr,
    };

    let wide: Vec<u16> = name.encode_wide().collect();
    let name_bytes = wide
        .len()
        .checked_mul(2)
        .ok_or_else(|| io::Error::other("name too long"))?;
    let name_bytes_u16 =
        u16::try_from(name_bytes).map_err(|_e| io::Error::other("name too long"))?;

    let unicode_name = UNICODE_STRING {
        Length: name_bytes_u16,
        MaximumLength: name_bytes_u16,
        Buffer: wide.as_ptr() as *mut u16,
    };

    let object_attrs = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent_handle,
        ObjectName: &unicode_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: &sd,
        SecurityQualityOfService: ptr::null(),
    };

    let mut new_handle: HANDLE = ptr::null_mut();
    let mut iosb = MaybeUninit::<IO_STATUS_BLOCK>::zeroed();

    // SAFETY: `new_handle` and `iosb` are valid out-pointers;
    // `object_attrs`, `unicode_name`, `sd`, `acl`, and `wide` all
    // live through the call. No EA buffer.
    let status = unsafe {
        NtCreateFile(
            &mut new_handle,
            desired_access,
            &object_attrs,
            iosb.as_mut_ptr(),
            ptr::null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_CREATE,
            create_options,
            ptr::null(),
            0,
        )
    };

    if status != STATUS_SUCCESS {
        return Err(nt_status_to_io_error(status));
    }

    Ok(new_handle)
}

/// fd-anchored rename. Moves the file behind `src`'s handle to
/// `dest_name` inside `dest_dir`'s handle. No path resolution:
/// the destination is identified entirely by the
/// `dest_dir.HANDLE` + `dest_name` pair, so an attacker who
/// substitutes the destination directory along its ambient path
/// cannot redirect the rename. `src` must have been opened with
/// `DELETE` access (see [`PAYLOAD_ACCESS_MASK`]).
///
/// POSIX semantics: if the destination exists, it is replaced
/// atomically; concurrent handles on the old destination are not
/// invalidated, matching `renameat` semantics on Unix.
pub(crate) fn rename_via_handle<S: AsRawHandle>(
    src: &S,
    dest_dir: &Dir,
    dest_name: &OsStr,
) -> io::Result<()> {
    let src_handle = src.as_raw_handle() as HANDLE;
    let dest_handle = dest_dir.as_raw_handle() as HANDLE;

    let wide: Vec<u16> = dest_name.encode_wide().collect();
    let name_bytes = wide
        .len()
        .checked_mul(2)
        .ok_or_else(|| io::Error::other("rename destination name too long"))?;
    let name_bytes_u32 = u32::try_from(name_bytes)
        .map_err(|_e| io::Error::other("rename destination name too long"))?;

    // FILE_RENAME_INFO's `FileName` is a flexible array trailing
    // a fixed header. The struct as declared reserves 2 bytes for
    // it; the kernel reads `FileNameLength` bytes starting at the
    // FileName offset, so the buffer total is
    // `offset_of(FileName) + name_bytes`.
    let header_size = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let total = header_size
        .checked_add(name_bytes)
        .ok_or_else(|| io::Error::other("rename buffer overflow"))?;
    let total_u32 =
        u32::try_from(total).map_err(|_e| io::Error::other("rename buffer too large"))?;

    // Vec<u8> doesn't guarantee enough alignment for HANDLE on
    // 64-bit. Vec<u64> gives 8-byte alignment which covers every
    // field of FILE_RENAME_INFO.
    let qwords = total.div_ceil(size_of::<u64>());
    let mut buf: Vec<u64> = vec![0; qwords];
    let info_ptr = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();

    // SAFETY: `info_ptr` points to `total` zeroed bytes; we
    // initialise every field before the kernel reads them.
    unsafe {
        (*info_ptr).Anonymous.Flags = FILE_RENAME_FLAGS;
        (*info_ptr).RootDirectory = dest_handle;
        (*info_ptr).FileNameLength = name_bytes_u32;
        let filename_dst = (info_ptr.cast::<u8>()).add(header_size).cast::<u16>();
        ptr::copy_nonoverlapping(wide.as_ptr(), filename_dst, wide.len());
    }

    // SAFETY: `src_handle` is owned by `src` and has DELETE
    // access; `info_ptr` points to a fully initialised
    // FILE_RENAME_INFO sized `total_u32` bytes.
    let ok = unsafe {
        SetFileInformationByHandle(src_handle, FileRenameInfoEx, info_ptr.cast(), total_u32)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Read the kernel object's owner SID via `GetSecurityInfo` and
/// confirm it matches the current process's token SID. With
/// [`create_owner_only_subdir`] the kernel sets ownership
/// atomically at create time, so this is defence-in-depth: it
/// catches filesystems that ignore security descriptors (FAT) and
/// any future regression where the kernel/cap-std diverged from
/// the expected ownership semantics.
pub(crate) fn verify_owner(dir: &Dir) -> io::Result<()> {
    let our_sid = owner_sid()?;

    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let mut owner: PSID = ptr::null_mut();
    let raw = dir.as_raw_handle() as HANDLE;

    // SAFETY: raw is a valid HANDLE owned by `dir`; the
    // out-pointers are valid; the descriptor allocated by
    // GetSecurityInfo is released by SecurityDescriptorGuard.
    let rc = unsafe {
        GetSecurityInfo(
            raw,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        )
    };
    let _guard = SecurityDescriptorGuard(sd);
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }

    // SAFETY: `owner` was populated by GetSecurityInfo and points
    // into the descriptor held by `_guard`; `our_sid` is the
    // cached process SID.
    let equal = unsafe { EqualSid(owner, our_sid.as_ptr() as PSID) } != 0;
    if !equal {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory ownership does not match current process",
        ));
    }
    Ok(())
}

/// Build an ACL with a single allow-ACE for `sid` carrying
/// `ace_flags`. Callers pass `OBJECT_INHERIT_ACE |
/// CONTAINER_INHERIT_ACE` for directories whose ACE should
/// propagate to children, or `0` for files (which have no
/// children and therefore no use for inheritance flags).
fn build_owner_only_acl(sid: &[u8], ace_flags: u32) -> io::Result<Vec<u8>> {
    if sid.len() < size_of::<u32>() {
        return Err(io::Error::other("owner SID too small"));
    }
    let ace_size = size_of::<ACCESS_ALLOWED_ACE>() + sid.len() - size_of::<u32>();
    let acl_size = size_of::<ACL>() + ace_size;
    let acl_size_u32 =
        u32::try_from(acl_size).map_err(|_e| io::Error::other("computed ACL size exceeds u32"))?;

    let mut buf = vec![0u8; acl_size];
    let acl_ptr = buf.as_mut_ptr().cast::<ACL>();

    // SAFETY: buf is `acl_size` bytes long; ACL_REVISION is the
    // documented revision constant.
    let ok = unsafe { InitializeAcl(acl_ptr, acl_size_u32, ACL_REVISION) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `acl_ptr` is a freshly initialised ACL with room
    // for one ACE referencing a SID of `sid.len()` bytes; `sid`
    // points to a validated SID.
    let ok = unsafe {
        AddAccessAllowedAceEx(
            acl_ptr,
            ACL_REVISION,
            ace_flags,
            FILE_ALL_ACCESS,
            sid.as_ptr() as PSID,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(buf)
}

/// Convert an `NTSTATUS` value to an [`io::Error`] by routing
/// through `RtlNtStatusToDosError`. This gives callers a
/// recognisable Win32 error kind (e.g. `AlreadyExists` for
/// `STATUS_OBJECT_NAME_COLLISION`) without each call site having
/// to know the NT codes.
fn nt_status_to_io_error(status: NTSTATUS) -> io::Error {
    // SAFETY: RtlNtStatusToDosError is a pure function; no
    // pointer arguments.
    let win32 = unsafe { RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(win32 as i32)
}

/// True if `err` is the Windows kernel reporting that a final
/// reparse point (symlink/junction) was refused. The exact
/// surface varies across configurations, so we match the two
/// reparse-specific codes and let everything else fall through
/// unchanged.
pub(crate) fn is_symlink_refusal(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(code)
            if code as u32 == ERROR_STOPPED_ON_SYMLINK
                || code as u32 == ERROR_CANT_ACCESS_FILE
    )
}

/// `LocalFree`s the `PSECURITY_DESCRIPTOR` returned by
/// `GetSecurityInfo` on drop.
struct SecurityDescriptorGuard(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptorGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: pointer was allocated by GetSecurityInfo
            // via LocalAlloc.
            unsafe { LocalFree(self.0.cast()) };
        }
    }
}

/// Read the owner SID of the file or directory behind `handle`.
/// Test-only inspector; production code uses [`verify_owner`]
/// which compares without copying.
#[cfg(test)]
pub(crate) fn read_owner_sid<H: AsRawHandle>(handle: &H) -> io::Result<Vec<u8>> {
    let raw = handle.as_raw_handle() as HANDLE;
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let mut owner: PSID = ptr::null_mut();
    // SAFETY: same contract as `verify_owner`.
    let rc = unsafe {
        GetSecurityInfo(
            raw,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        )
    };
    let _guard = SecurityDescriptorGuard(sd);
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    // SAFETY: `owner` was populated by GetSecurityInfo and is
    // valid for the duration of `_guard`.
    let len = unsafe { GetLengthSid(owner) } as usize;
    let mut out = vec![0u8; len];
    // SAFETY: `out` is `len` bytes long; `owner` is a valid SID.
    let ok = unsafe { CopySid(len as u32, out.as_mut_ptr().cast(), owner) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(out)
}

/// True if the DACL on `handle`'s object has the
/// `SE_DACL_PROTECTED` control bit set (i.e. no inheritable ACEs
/// from the parent apply). Test-only.
#[cfg(test)]
pub(crate) fn dacl_is_protected<H: AsRawHandle>(handle: &H) -> io::Result<bool> {
    let raw = handle.as_raw_handle() as HANDLE;
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: out-pointers are valid; descriptor allocated by the
    // kernel is freed by `_guard`.
    let rc = unsafe {
        GetSecurityInfo(
            raw,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        )
    };
    let _guard = SecurityDescriptorGuard(sd);
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    // SAFETY: sd is valid for the duration of `_guard`; both
    // out-pointers are valid.
    let ok = unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((control & SE_DACL_PROTECTED) != 0)
}
