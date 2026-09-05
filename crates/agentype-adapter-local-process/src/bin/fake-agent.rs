//! Test fixture external agent environment. Behavior is selected by env vars.
//! This is not a model, provider, or harness.

use std::env;
use std::io::{self, Read};
use std::thread;
use std::time::Duration;

fn flag(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
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

fn main() {
    if flag("FAKE_AGENT_TRAP_INT") {
        trap_interrupt();
    }

    // Optional delay after process creation (pid already assigned).
    sleep_ms_from_env();

    if flag("FAKE_AGENT_HANG") {
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

    if flag("FAKE_AGENT_MALFORMED") {
        println!("this is not json {{{{{{");
        return;
    }

    if let Ok(stdout) = env::var("FAKE_AGENT_STDOUT") {
        println!("{stdout}");
    } else {
        println!(r#"{{"ok":true,"payload":{{"echo":true}},"summary":"fake-agent"}}"#);
    }

    let code = env::var("FAKE_AGENT_EXIT_CODE")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
