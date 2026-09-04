//! Handle-based identity and deletion for protected Windows product state.
//!
//! Keeping these few calls in the existing sanctioned FFI boundary avoids
//! unsafe code in the product. Callers own content/ACL policy; this module pins
//! the path ancestors and deletes only the exact open handle they verified.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::windows::io::AsRawHandle as _;
use std::path::{Component, Path};

use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo, FileIdInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, SetFileInformationByHandle,
};

/// Filesystem identity, including the 128-bit ID required by ReFS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume: u64,
    pub id: [u8; 16],
    pub links: u32,
}

/// Keeps every existing ancestor unrenamable while a path operation is active.
pub struct AncestorGuard {
    directories: Vec<File>,
}

impl AncestorGuard {
    /// Pin an absolute file path's ancestors, rejecting junctions and symlinks.
    pub fn for_file(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() || path.components().any(|part| {
            matches!(part, Component::ParentDir)
                || matches!(part, Component::Normal(name) if name.to_string_lossy().contains(':'))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected absolute file path",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("file has no parent"))?;
        let mut directories = Vec::new();
        let ancestors: Vec<_> = parent.ancestors().collect();
        for directory in ancestors.into_iter().rev() {
            // Path::ancestors can expose a prefix without its root separator.
            if !directory.has_root()
                || directory
                    .components()
                    .all(|part| matches!(part, Component::Prefix(_)))
            {
                continue;
            }
            let file = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(directory)?;
            let metadata = file.metadata()?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(io::Error::other(
                    "protected path contains a reparse point or non-directory",
                ));
            }
            directories.push(file);
        }
        if directories.is_empty() {
            return Err(io::Error::other("file has no pinned parent"));
        }
        Ok(Self { directories })
    }

    /// Identity of the pinned immediate parent.
    pub fn parent_identity(&self) -> io::Result<FileIdentity> {
        identity(
            self.directories.last().expect("guard always owns a parent"),
            true,
        )
    }

    /// Verify the immediate parent's write/delete-child authority before a
    /// caller creates, reads, replaces, or deletes protected artifacts.
    pub fn verify_parent_acl(&self) -> io::Result<()> {
        verify_directory_acl(self.directories.last().expect("guard always owns a parent"))
    }
}

/// Open a regular file without following its final reparse point. While held,
/// other handles cannot write, replace, or delete the verified file.
pub fn open_read(path: &Path, delete_access: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ | if delete_access { DELETE } else { 0 })
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    identity(&file, false)?;
    Ok(file)
}

/// Open a directory without following or permitting replacement of its entry.
pub fn open_directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    identity(&file, true)?;
    Ok(file)
}

/// Query the exact open object's volume, full file ID, and hard-link count.
pub fn identity(file: &File, directory: bool) -> io::Result<FileIdentity> {
    let mut basic = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let mut full = std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    // SAFETY: live borrowed File owns the handle; both output buffers have the
    // exact documented layouts and remain valid until the synchronous calls end.
    unsafe {
        if GetFileInformationByHandle(file.as_raw_handle(), basic.as_mut_ptr()) == 0 {
            return Err(io::Error::last_os_error());
        }
        let basic = basic.assume_init();
        if basic.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (basic.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && basic.nNumberOfLinks != 1)
        {
            return Err(io::Error::other(
                "protected object has unsafe type or hard links",
            ));
        }
        if GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            full.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let full = full.assume_init();
        Ok(FileIdentity {
            volume: full.VolumeSerialNumber,
            id: full.FileId.Identifier,
            links: basic.nNumberOfLinks,
        })
    }
}

/// Mark this verified handle for deletion, never resolving its pathname again.
/// The caller must have opened it with delete access and release it afterward.
pub fn delete_on_close(file: &File) -> io::Result<()> {
    identity(file, false)?;
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: the borrowed handle is live and the input buffer is valid with
    // the exact layout/size required by FileDispositionInfo.
    let result = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Verify a protected, sole-current-user file DACL without changing it.
pub fn verify_private_acl(file: &File) -> io::Result<()> {
    verify_acl(file, false)
}

/// Refuse parent directories writable by anyone except the current user and
/// Windows' trusted SYSTEM/Administrators principals. Existing directories may
/// inherit trusted rules; they are never rewritten to make them acceptable.
pub fn verify_directory_acl(file: &File) -> io::Result<()> {
    verify_acl(file, true)
}

fn with_current_user<T>(
    operation: impl FnOnce(windows_sys::Win32::Security::PSID) -> io::Result<T>,
) -> io::Result<T> {
    use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: the owned process token and aligned, bounded TOKEN_USER buffer
    // remain live throughout the callback. No SID escapes this private helper.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut buffer = [0_usize; 128];
        let mut returned = 0;
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            std::mem::size_of_val(&buffer) as u32,
            &mut returned,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if returned < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::other("invalid process token identity"));
        }
        operation((*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid)
    }
}

/// Assign current-user ownership only to a caller's newly-created object.
/// The path is pinned, and the re-opened handle must match the original file ID.
pub fn set_created_owner(path: &Path, original: &File, directory: bool) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
    use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::WRITE_OWNER;
    let _parents = AncestorGuard::for_file(path)?;
    let expected = identity(original, directory)?;
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ | WRITE_OWNER)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        )
        .open(path)?;
    if identity(&file, directory)? != expected {
        return Err(io::Error::other("created object identity changed"));
    }
    with_current_user(|user| {
        // SAFETY: the borrowed file handle and process-owned SID remain live.
        let status = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                user,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    })
}

fn verify_acl(file: &File, directory: bool) -> io::Result<()> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetSecurityDescriptorControl, IsValidAcl, IsValidSid, IsWellKnownSid,
        OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED, WinBuiltinAdministratorsSid,
        WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES,
        FILE_WRITE_DATA, FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER,
    };
    struct Descriptor(*mut std::ffi::c_void);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            // SAFETY: GetSecurityInfo allocated this buffer with LocalAlloc.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "protected object owner or ACL is unsafe",
        )
    };
    with_current_user(|user| {
        // SAFETY: GetSecurityInfo owns a valid descriptor; every ACE is checked
        // for its header/type/length before reading its variable-length SID.
        // The descriptor and token buffer remain owned until all SID reads end.
        unsafe {
            let mut owner: PSID = std::ptr::null_mut();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut descriptor = std::ptr::null_mut();
            let status = GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            );
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let _descriptor = Descriptor(descriptor);
            let trusted = |sid| {
                EqualSid(sid, user) != 0
                    || (directory
                        && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0))
            };
            if descriptor.is_null()
                || owner.is_null()
                || dacl.is_null()
                || IsValidSid(owner) == 0
                || IsValidAcl(dacl) == 0
                || !trusted(owner)
            {
                return Err(denied());
            }
            let mut control = 0;
            let mut revision = 0;
            if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0 {
                return Err(io::Error::last_os_error());
            }
            if !directory && control & SE_DACL_PROTECTED == 0 {
                return Err(denied());
            }
            let mut size = std::mem::MaybeUninit::<ACL_SIZE_INFORMATION>::uninit();
            if GetAclInformation(
                dacl,
                size.as_mut_ptr().cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let count = size.assume_init().AceCount;
            if count > 1024 || (!directory && count != 1) {
                return Err(denied());
            }
            for index in 0..count {
                let mut ace = std::ptr::null_mut();
                if GetAce(dacl, index, &mut ace) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let header = &*ace.cast::<windows_sys::Win32::Security::ACE_HEADER>();
                if directory && header.AceType == 1 {
                    continue;
                } // deny cannot grant foreign access
                if header.AceType != 0
                    || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
                {
                    return Err(denied());
                }
                if !directory && header.AceFlags & 0x18 != 0 {
                    return Err(denied());
                } // INHERIT_ONLY / INHERITED
                if directory && header.AceFlags & 0x08 != 0 {
                    continue;
                } // does not apply to parent itself
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                let sid_bytes = std::ptr::addr_of!(allowed.SidStart).cast::<u8>();
                let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
                if usize::from(header.AceSize) < sid_offset + 8 {
                    return Err(denied());
                }
                let sid_len = 8 + usize::from(*sid_bytes.add(1)) * 4;
                if usize::from(header.AceSize) < sid_offset + sid_len {
                    return Err(denied());
                }
                let sid = sid_bytes.cast_mut().cast();
                if IsValidSid(sid) == 0 {
                    return Err(denied());
                }
                if directory {
                    // Generic masks are included as well as expanded file rights.
                    let writes = FILE_WRITE_DATA
                        | FILE_APPEND_DATA
                        | FILE_WRITE_EA
                        | FILE_WRITE_ATTRIBUTES
                        | FILE_DELETE_CHILD
                        | DELETE
                        | WRITE_DAC
                        | WRITE_OWNER
                        | 0x5000_0000;
                    if allowed.Mask & writes != 0 && !trusted(sid) {
                        return Err(denied());
                    }
                } else if EqualSid(sid, user) == 0
                    || allowed.Mask & FILE_ALL_ACCESS != FILE_ALL_ACCESS
                {
                    return Err(denied());
                }
            }
            Ok(())
        }
    })
}
