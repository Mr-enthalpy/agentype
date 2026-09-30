//! Handle-derived file identity and hard-link count.
//!
//! Unix uses stable metadata. Windows uses OS calls so `agentype-runtime`
//! can keep `forbid(unsafe_code)`. Identity is never resolved by re-opening
//! a path.

use std::fs::File;
use std::io;
#[cfg(windows)]
use std::path::PathBuf;

pub fn link_count(file: &File) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink())
    }
    #[cfg(windows)]
    {
        link_count_windows(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(1)
    }
}

#[cfg(windows)]
fn link_count_windows(file: &File) -> io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(info.nNumberOfLinks))
}

/// Identity of the file this handle already refers to. Stable across rename.
pub fn file_identity(file: &File) -> io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        Ok(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(windows)]
    {
        file_identity_windows(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file identity is unavailable on this platform",
        ))
    }
}

#[cfg(windows)]
fn file_identity_windows(file: &File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(format!("{}:{index}", info.dwVolumeSerialNumber))
}

/// Machine-wide directory for file-identity locks. Not `Local\`, and not
/// any path derived from the database's current parent or from environment
/// variables such as `PROGRAMDATA`.
#[cfg(windows)]
pub fn machine_lock_dir() -> io::Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

    let mut raw = std::ptr::null_mut();
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        return Err(io::Error::other(format!(
            "ProgramData known folder is unavailable ({hr})"
        )));
    }
    let mut len = 0usize;
    unsafe {
        while *raw.add(len) != 0 {
            len += 1;
        }
    }
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(path).join("agentype").join("locks"))
}
