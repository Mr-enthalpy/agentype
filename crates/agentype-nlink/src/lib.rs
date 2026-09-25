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
