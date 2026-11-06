//! Test fixture external agent environment. Behavior is selected by arguments
//! or, for legacy fixtures, by env vars. This is not a model, provider, or
//! harness.
//!
//! Argument-selected behavior exists because a workspace-jail worker runs with
//! an explicit minimal environment on every platform, so tests cannot select
//! behavior through the daemon's inherited environment.

use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

fn flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

fn arg_flag(name: &str) -> bool {
    env::args().skip(1).any(|arg| arg == name)
}

fn arg_value(name: &str) -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}

fn arg_values(name: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            if let Some(value) = args.next() {
                values.push(value);
            }
        }
    }
    values
}

fn sleep_ms_from_env() {
    if let Ok(ms) = env::var("FAKE_AGENT_SLEEP_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            thread::sleep(Duration::from_millis(ms));
        }
    }
}

fn hang() -> ! {
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn trap_interrupt() {
    #[cfg(unix)]
    {
        const SIGINT: i32 = 2;
        const SIG_IGN: usize = 1;
        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        unsafe {
            let _ = signal(SIGINT, SIG_IGN);
        }
    }
    #[cfg(windows)]
    {
        unsafe extern "system" fn ignore(_ctrl: u32) -> i32 {
            1
        }
        extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(ignore), 1);
        }
    }
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Attempt one write and report the outcome as a single JSON line. Tests read
/// this from the adapter-captured stdout, so a denial is observable without
/// relying on exit codes or diagnostics.
fn write_probe(path: &str, label: &str) {
    let target = PathBuf::from(path);
    let result = fs::write(&target, b"agentype-write-probe");
    match result {
        Ok(()) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":true}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy())
        ),
        Err(err) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":false,"kind":"{}"}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy()),
            json_escape(&format!("{:?}", err.kind()))
        ),
    }
}

fn remove_probe(path: &Path, label: &str) {
    match fs::remove_file(path) {
        Ok(()) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":true}}}}"#,
            json_escape(label),
            json_escape(&path.to_string_lossy())
        ),
        Err(err) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":false,"kind":"{}"}}}}"#,
            json_escape(label),
            json_escape(&path.to_string_lossy()),
            json_escape(&format!("{:?}", err.kind()))
        ),
    }
}

/// Truncate an existing file through a normal write handle (`ftruncate`).
fn truncate_probe(path: &str, label: &str) {
    let target = PathBuf::from(path);
    let result = (|| {
        let file = fs::OpenOptions::new().write(true).open(&target)?;
        file.set_len(0)
    })();
    match result {
        Ok(()) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":true}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy())
        ),
        Err(err) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":false,"kind":"{}"}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy()),
            json_escape(&format!("{:?}", err.kind()))
        ),
    }
}

/// Truncate via `open(path, O_RDONLY | O_TRUNC)`, which does not request
/// `WRITE_FILE` and is therefore the Landlock ABI < 3 bypass.
#[cfg(target_os = "linux")]
fn open_truncate_probe(path: &str, label: &str) {
    use std::os::unix::ffi::OsStrExt;

    let target = PathBuf::from(path);
    let result = (|| -> io::Result<()> {
        let raw = std::ffi::CString::new(target.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        // SAFETY: `raw` is a valid NUL-terminated path.
        let fd = unsafe { libc::open(raw.as_ptr(), libc::O_RDONLY | libc::O_TRUNC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is owned by this frame.
        unsafe {
            libc::close(fd);
        }
        Ok(())
    })();
    match result {
        Ok(()) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":true}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy())
        ),
        Err(err) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":false,"kind":"{}"}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy()),
            json_escape(&format!("{:?}", err.kind()))
        ),
    }
}

/// Exercise metadata syscalls that Landlock does not mediate: `chmod`,
/// `utimensat`, and `setxattr`. Used by real Linux tests to keep the
/// content-write enforcement claim honest: the documentation states that
/// metadata changes are not restricted, so a positive result here is expected.
#[cfg(target_os = "linux")]
fn metadata_probe(path: &str) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let target = PathBuf::from(path);
    let Ok(raw) = CString::new(target.as_os_str().as_bytes()) else {
        return;
    };
    let report = |label: &str, result: io::Result<()>| match result {
        Ok(()) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":true,"errno":0}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy())
        ),
        Err(err) => println!(
            r#"{{"probe":{{"label":"{}","path":"{}","ok":false,"errno":{}}}}}"#,
            json_escape(label),
            json_escape(&target.to_string_lossy()),
            err.raw_os_error().unwrap_or(0)
        ),
    };
    let syscall = |rc: i32| -> io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    };

    // SAFETY: raw is a valid NUL-terminated path.
    report("chmod", unsafe {
        syscall(libc::chmod(raw.as_ptr(), 0o600))
    });
    // SAFETY: raw is valid; a null times pointer requests the current time.
    report("utimensat", unsafe {
        syscall(libc::utimensat(
            libc::AT_FDCWD,
            raw.as_ptr(),
            std::ptr::null(),
            0,
        ))
    });
    let name = b"user.agentype_probe\0";
    // SAFETY: raw and name are valid NUL-terminated byte strings; the value
    // buffer is valid for the given length.
    report("setxattr", unsafe {
        syscall(libc::setxattr(
            raw.as_ptr(),
            name.as_ptr().cast(),
            b"1".as_ptr().cast(),
            1,
            0,
        ))
    });
}

/// Report whether an environment variable is visible. The value is never
/// printed so a sentinel secret cannot leak into the captured artifact.
fn env_probe(name: &str) {
    let present = env::var_os(name).is_some();
    println!(
        r#"{{"probe":{{"label":"env","name":"{}","present":{present},"ok":{present}}}}}"#,
        json_escape(name)
    );
}

/// Report the worker's effective token privileges and window desktop, and
/// attempt to open the interactive `Default` and input desktops. A restricted
/// token must not retain host privileges and must not reach either desktop.
#[cfg(windows)]
fn isolation_probe() {
    use std::os::windows::ffi::OsStrExt;

    type Handle = *mut core::ffi::c_void;
    type Dword = u32;
    type Bool = i32;

    #[repr(C)]
    struct Luid {
        low: Dword,
        high: i32,
    }

    #[repr(C)]
    struct LuidAndAttributes {
        luid: Luid,
        attributes: Dword,
    }

    #[repr(C)]
    struct TokenPrivileges {
        count: Dword,
        privileges: [LuidAndAttributes; 1],
    }

    /// `Vec<u8>` only guarantees byte alignment; TOKEN_PRIVILEGES must be read
    /// from storage aligned for its fields. Eight-byte words cover that.
    #[repr(C, align(8))]
    #[derive(Clone, Copy)]
    struct TokenWord([u8; 8]);

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn GetCurrentThreadId() -> Dword;
        fn CloseHandle(handle: Handle) -> Bool;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: Dword, token: *mut Handle) -> Bool;
        fn GetTokenInformation(
            token: Handle,
            class: Dword,
            info: *mut core::ffi::c_void,
            len: Dword,
            needed: *mut Dword,
        ) -> Bool;
        fn LookupPrivilegeNameW(
            system: *const u16,
            luid: *const Luid,
            name: *mut u16,
            len: *mut Dword,
        ) -> Bool;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetThreadDesktop(thread_id: Dword) -> Handle;
        fn GetProcessWindowStation() -> Handle;
        fn GetUserObjectInformationW(
            object: Handle,
            index: i32,
            info: *mut core::ffi::c_void,
            len: Dword,
            needed: *mut Dword,
        ) -> Bool;
        fn OpenDesktopW(
            name: *const u16,
            flags: Dword,
            desired_access: Dword,
            inherit: Bool,
        ) -> Handle;
        fn OpenInputDesktop(flags: Dword, inherit: Bool, desired_access: Dword) -> Handle;
        fn OpenWindowStationW(name: *const u16, inherit: Bool, access: Dword) -> Handle;
        fn CloseDesktop(desktop: Handle) -> Bool;
        fn CloseWindowStation(station: Handle) -> Bool;
    }

    const TOKEN_QUERY: Dword = 0x0008;
    const TOKEN_PRIVILEGES_CLASS: Dword = 3;
    const UOI_NAME: i32 = 2;
    const DESKTOP_READOBJECTS: Dword = 0x0001;
    const WINSTA_ENUMDESKTOPS: Dword = 0x0001;
    const WINSTA_READATTRIBUTES: Dword = 0x0002;
    const WINSTA_ACCESSCLIPBOARD: Dword = 0x0004;
    const WINSTA_CREATEDESKTOP: Dword = 0x0008;
    const WINSTA_WRITEATTRIBUTES: Dword = 0x0010;
    const WINSTA_ACCESSGLOBALATOMS: Dword = 0x0020;
    const WINSTA_EXITWINDOWS: Dword = 0x0040;
    const WINSTA_ENUMERATE: Dword = 0x0100;
    const WINSTA_READSCREEN: Dword = 0x0200;
    const WINSTA_ALL_ACCESS: Dword = 0x037F;
    const STANDARD_RIGHTS_REQUIRED: Dword = 0x000F_0000;

    let user_object_name = |handle: Handle| -> Option<String> {
        if handle.is_null() {
            return None;
        }
        let mut needed: Dword = 0;
        // SAFETY: size probe with a null buffer.
        unsafe {
            GetUserObjectInformationW(handle, UOI_NAME, std::ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (needed as usize).div_ceil(2)];
        // SAFETY: buffer has at least `needed` bytes.
        let ok = unsafe {
            GetUserObjectInformationW(
                handle,
                UOI_NAME,
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return None;
        }
        let len = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    };

    let mut privileges = Vec::new();
    let mut token: Handle = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess is a pseudo-handle; token is an out parameter.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0 {
        let mut needed: Dword = 0;
        // SAFETY: size probe with a null buffer.
        unsafe {
            GetTokenInformation(
                token,
                TOKEN_PRIVILEGES_CLASS,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        if needed > 0 {
            let bytes = needed as usize;
            let words = bytes.div_ceil(8);
            let mut buffer = vec![TokenWord([0u8; 8]); words];
            let capacity = u32::try_from(words.saturating_mul(8)).unwrap_or(u32::MAX);
            // SAFETY: buffer is word-aligned and at least `needed` bytes;
            // `needed` is an out parameter.
            let ok = unsafe {
                GetTokenInformation(
                    token,
                    TOKEN_PRIVILEGES_CLASS,
                    buffer.as_mut_ptr().cast(),
                    capacity,
                    &mut needed,
                )
            };
            if ok != 0 {
                assert!(
                    std::mem::align_of::<TokenPrivileges>() <= std::mem::align_of::<TokenWord>(),
                    "the token buffer word alignment must satisfy TOKEN_PRIVILEGES alignment"
                );
                // SAFETY: the buffer is at least `needed` bytes and word
                // aligned; the API wrote a TOKEN_PRIVILEGES header at its base.
                let token_privileges = unsafe { &*buffer.as_ptr().cast::<TokenPrivileges>() };
                let count = token_privileges.count as usize;
                let header = std::mem::size_of::<TokenPrivileges>()
                    - std::mem::size_of::<LuidAndAttributes>();
                let entry = std::mem::size_of::<LuidAndAttributes>();
                let required = header.saturating_add(count.saturating_mul(entry));
                if required <= needed as usize && required <= capacity as usize {
                    // SAFETY: the count was validated against the bytes the
                    // API actually returned inside this allocation.
                    let entries = unsafe {
                        std::slice::from_raw_parts(token_privileges.privileges.as_ptr(), count)
                    };
                    for entry in entries {
                        let mut name = vec![0u16; 256];
                        let mut len = name.len() as Dword;
                        // SAFETY: name is a valid out buffer; entry.luid is valid.
                        if unsafe {
                            LookupPrivilegeNameW(
                                std::ptr::null(),
                                &entry.luid,
                                name.as_mut_ptr(),
                                &mut len,
                            )
                        } != 0
                        {
                            let end = name
                                .iter()
                                .position(|&unit| unit == 0)
                                .unwrap_or(len as usize);
                            privileges.push(String::from_utf16_lossy(&name[..end]));
                        }
                    }
                }
            }
        }
        // SAFETY: token is owned by this frame.
        unsafe {
            CloseHandle(token);
        }
    }

    let desktop = user_object_name(unsafe { GetThreadDesktop(GetCurrentThreadId()) });
    let winsta = user_object_name(unsafe { GetProcessWindowStation() });
    let default_name: Vec<u16> = std::ffi::OsStr::new("Default")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: default_name is NUL terminated; a failed open is the expected
    // isolation result rather than an error to report.
    let default_desktop = unsafe { OpenDesktopW(default_name.as_ptr(), 0, DESKTOP_READOBJECTS, 0) };
    let default_desktop_open = !default_desktop.is_null();
    if default_desktop_open {
        unsafe {
            CloseDesktop(default_desktop);
        }
    }
    // SAFETY: a failed open is the expected isolation result.
    let input_desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
    let input_desktop_open = !input_desktop.is_null();
    if input_desktop_open {
        unsafe {
            CloseDesktop(input_desktop);
        }
    }

    // A real access check against the current window station DACL. The token is
    // write-restricted: the restricting SIDs are applied to write-shaped access
    // checks, while read-shaped rights are evaluated with the token's normal
    // SIDs against the station's pre-existing DACL. The adapter grants the
    // restricting SID no station ACE at all, so it cannot be the source of a
    // write grant such as CREATEDESKTOP, WRITEATTRIBUTES, ACCESSCLIPBOARD,
    // EXITWINDOWS, or ACCESSGLOBALATOMS. A combined request proves that adding a
    // read right does not smuggle in a write right.
    let window_station_access = |access: Dword| -> bool {
        let Some(name) = winsta.as_deref() else {
            return false;
        };
        let name: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: name is NUL terminated. A failed open is the expected result
        // for rights the adapter deliberately did not grant.
        let station = unsafe { OpenWindowStationW(name.as_ptr(), 0, access) };
        if station.is_null() {
            return false;
        }
        unsafe {
            CloseWindowStation(station);
        }
        true
    };
    let can_enum_desktops = window_station_access(WINSTA_ENUMDESKTOPS);
    let can_read_attributes = window_station_access(WINSTA_READATTRIBUTES);
    let can_enumerate = window_station_access(WINSTA_ENUMERATE);
    let can_create_desktop = window_station_access(WINSTA_CREATEDESKTOP);
    let can_write_attributes = window_station_access(WINSTA_WRITEATTRIBUTES);
    let can_access_clipboard = window_station_access(WINSTA_ACCESSCLIPBOARD);
    let can_access_global_atoms = window_station_access(WINSTA_ACCESSGLOBALATOMS);
    let can_exit_windows = window_station_access(WINSTA_EXITWINDOWS);
    // READSCREEN is a read-shaped right: under a write-restricted token it is
    // governed by the station's existing DACL (which grants the interactive
    // session read access), not by the adapter's minimal write ACE.
    let can_read_screen = window_station_access(WINSTA_READSCREEN);
    let readscreen_plus_create_desktop =
        window_station_access(WINSTA_READSCREEN | WINSTA_CREATEDESKTOP);
    let has_standard_rights = window_station_access(STANDARD_RIGHTS_REQUIRED);
    let has_all_access = window_station_access(WINSTA_ALL_ACCESS);

    let privileges_json = privileges
        .iter()
        .map(|privilege| format!("\"{}\"", json_escape(privilege)))
        .collect::<Vec<_>>()
        .join(",");
    println!(
        r#"{{"probe":{{"label":"isolation","ok":true,"privileges":[{privileges_json}],"desktop":"{}","winsta":"{}","default_desktop_open":{default_desktop_open},"input_desktop_open":{input_desktop_open},"winsta_access":{{"enumdesktops":{can_enum_desktops},"readattributes":{can_read_attributes},"enumerate":{can_enumerate},"createdesktop":{can_create_desktop},"writeattributes":{can_write_attributes},"accessclipboard":{can_access_clipboard},"accessglobalatoms":{can_access_global_atoms},"exitwindows":{can_exit_windows},"readscreen":{can_read_screen},"readscreen_plus_createdesktop":{readscreen_plus_create_desktop},"standard_rights_required":{has_standard_rights},"all_access":{has_all_access}}}}}}}"#,
        json_escape(desktop.as_deref().unwrap_or("")),
        json_escape(winsta.as_deref().unwrap_or("")),
    );
}

fn main() {
    if flag("FAKE_AGENT_TRAP_INT") || arg_flag("--trap-int") {
        trap_interrupt();
    }

    // Optional delay after process creation (pid already assigned).
    if let Some(ms) = arg_value("--sleep-ms").and_then(|raw| raw.parse::<u64>().ok()) {
        thread::sleep(Duration::from_millis(ms));
    } else {
        sleep_ms_from_env();
    }

    if let Some(path) = arg_value("--ready-probe") {
        write_probe(&path, "ready");
    }

    if flag("FAKE_AGENT_HANG") || arg_flag("--hang") {
        hang();
    }

    if flag("FAKE_AGENT_HANG_ON_STDIN") {
        let mut one = [0u8; 1];
        let _ = io::stdin().read(&mut one);
        hang();
    }

    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);

    if flag("FAKE_AGENT_HANG_AFTER_STDIN") {
        hang();
    }

    if let Ok(secret) = env::var("FAKE_AGENT_STDERR_SECRET") {
        eprintln!("{secret}");
    }

    if let Some(path) = arg_value("--write-probe") {
        write_probe(&path, "write");
    }

    if arg_flag("--hang-after-write") {
        hang();
    }

    if let Some(path) = arg_value("--truncate-probe") {
        truncate_probe(&path, "truncate");
    }

    #[cfg(target_os = "linux")]
    if let Some(path) = arg_value("--open-truncate-probe") {
        open_truncate_probe(&path, "open-truncate");
    }

    #[cfg(target_os = "linux")]
    if let Some(path) = arg_value("--metadata-probe") {
        metadata_probe(&path);
    }

    for name in arg_values("--env-probe") {
        env_probe(&name);
    }

    if let Some(path) = arg_value("--remove-probe") {
        remove_probe(Path::new(&path), "remove");
    }

    #[cfg(windows)]
    if arg_flag("--isolation-probe") {
        isolation_probe();
    }

    if flag("FAKE_AGENT_MALFORMED") || arg_flag("--malformed") {
        println!("this is not json {{{{{{{{");
        return;
    }

    if let Some(stdout) = arg_value("--stdout") {
        println!("{stdout}");
    } else if let Ok(stdout) = env::var("FAKE_AGENT_STDOUT") {
        println!("{stdout}");
    } else {
        println!(r#"{{"ok":true,"payload":{{"echo":true}},"summary":"fake-agent"}}"#);
    }

    let code = arg_value("--exit-code")
        .or_else(|| env::var("FAKE_AGENT_EXIT_CODE").ok())
        .and_then(|raw| raw.parse::<i32>().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
