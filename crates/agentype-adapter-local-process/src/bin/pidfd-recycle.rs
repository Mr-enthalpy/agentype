//! Linux-only: exec target for `unshare --pid --fork` recycle proof.
//! Single-threaded `main` so `fork` after exec is safe.

fn main() {
    #[cfg(target_os = "linux")]
    {
        std::process::exit(agentype_adapter_local_process::pidfd_recycle_experiment());
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::exit(0);
    }
}
