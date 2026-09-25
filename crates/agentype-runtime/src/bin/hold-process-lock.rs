//! Test helper: acquire a RuntimeProcessLock and wait until stdin closes.

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: hold-process-lock <sqlite-path>");
    agentype_runtime::hold_process_lock_until_stdin_closes(std::path::Path::new(&path))
        .unwrap_or_else(|err| {
            eprintln!("{err}");
            std::process::exit(1);
        });
}
