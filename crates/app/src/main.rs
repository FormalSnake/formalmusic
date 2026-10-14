#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

/// glibc gives each thread that allocates under contention an arena of its own and rarely hands
/// freed memory in them back; two arenas keep RSS flat while artwork decodes on worker threads.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn cap_malloc_arenas() {
    const M_ARENA_MAX: std::ffi::c_int = -8;
    unsafe extern "C" {
        fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
    }
    // Safety: called first thing in `main`, before any other thread exists.
    unsafe {
        mallopt(M_ARENA_MAX, 2);
    }
}

fn main() -> nd::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    cap_malloc_arenas();
    formalmusic::run()
}
