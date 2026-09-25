//! Hard-link count. Unix uses stable metadata; Windows uses one OS call
//! so `agentype-runtime` can keep `forbid(unsafe_code)`.

use std::fs::File;
use std::io;

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

/// Crash-released exclusive ownership of one file identity.
///
/// Windows `LockFile` on the Scheduler database collides with SQLite, and a
/// lock file next to the current path splits after a same-volume rename.
/// The mutex name is the file id, so the contention follows the file.
#[cfg(windows)]
pub struct IdentityMutex {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for IdentityMutex {}

#[cfg(windows)]
impl Drop for IdentityMutex {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.handle);
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// `Some` owns the mutex. `None` means another process already owns this file id.
#[cfg(windows)]
pub fn try_acquire_identity_mutex(file_id: &str) -> io::Result<Option<IdentityMutex>> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, WAIT_ABANDONED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

    let wide = mutex_name(file_id);
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let created = unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
    if created {
        return Ok(Some(IdentityMutex { handle }));
    }
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        return Ok(Some(IdentityMutex { handle }));
    }
    unsafe { CloseHandle(handle) };
    if wait == WAIT_TIMEOUT {
        return Ok(None);
    }
    Err(io::Error::last_os_error())
}

#[cfg(windows)]
fn mutex_name(file_id: &str) -> Vec<u16> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut name = String::from("Local\\agentype-file-");
    for byte in file_id.as_bytes() {
        name.push(HEX[(byte >> 4) as usize] as char);
        name.push(HEX[(byte & 0xf) as usize] as char);
    }
    name.encode_utf16().chain(std::iter::once(0)).collect()
}
