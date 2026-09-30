//! Test helper: acquire a RuntimeProcessLock and wait until stdin closes.
//!
//! The helper is only useful to a cross-process test, so it lives here rather
//! than on the production API surface. It takes OS ownership of a store and
//! can hold it for the lifetime of the process; nothing a production consumer
//! should be able to call.

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: hold-process-lock <sqlite-path>");
    let config =
        agentype_runtime::SqliteRuntimeConfig::new(std::path::Path::new(&path), 10.0, 16_384)
            .unwrap_or_else(|err| {
                eprintln!("{err}");
                std::process::exit(1);
            });
    let _guard = agentype_runtime::RuntimeProcessGuard::acquire(&config).unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });
    {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        writeln!(stdout, "LOCKED").expect("helper stdout");
        stdout.flush().expect("helper stdout");
    }
    let mut sink = String::new();
    let _ = std::io::stdin().read_line(&mut sink);
}
