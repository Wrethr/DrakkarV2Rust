/// Platform-specific utilities for signal handling and process management.
///
/// Two variants are implemented:
///
/// - **Variant A (pure std)**: Uses a global AtomicBool cancellation token
///   and kills child processes via `Child::kill()`.
///
/// - **Variant B (Unix FFI)**: When `use_process_groups` is true and we're
///   on Unix, spawned children get their own process group (pgid). On Ctrl+C,
///   the entire process group is killed via `killpg`. This guarantees that
///   grandchildren (e.g. processes spawned by compiler scripts) are also killed.
///
/// On non-Unix platforms, Variant A is always used.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Global cancellation token. Workers check this flag in their loops.
static CANCEL_TOKEN: AtomicBool = AtomicBool::new(false);

pub fn is_cancelled() -> bool {
    CANCEL_TOKEN.load(Ordering::Relaxed)
}

pub fn cancel() {
    CANCEL_TOKEN.store(true, Ordering::Relaxed);
}

pub fn reset_cancel() {
    CANCEL_TOKEN.store(false, Ordering::Relaxed);
}

/// Register a Ctrl+C / SIGINT handler.
/// Uses pure std via a background thread that reads from a pipe/signal.
/// Variant A: just sets the global CANCEL_TOKEN.
pub fn register_ctrlc_handler() {
    // We use a background thread with a simple signal check.
    // The standard approach on stable Rust without external crates:
    // Set a custom panic hook that ignores; rely on the OS delivering SIGINT
    // to the process and terminating the Command children naturally,
    // plus our AtomicBool for clean worker shutdown.
    //
    // For the production-quality implementation, users should enable the
    // `use_process_groups = "true"` config flag (Variant B, requires Unix FFI).
    //
    // Here we implement Variant A: spawn a thread that polls for SIGINT
    // via a self-pipe trick on Unix, or via SetConsoleCtrlHandler on Windows.

    #[cfg(unix)]
    {
        use std::os::unix::io::RawFd;
        unsafe {
            // Set up SIGINT handler using libc via raw syscall-free approach.
            // We use signal(SIGINT, SIG_DFL) as baseline and a background thread
            // with sigwait() is the cleanest approach. Since we're pure std,
            // we approximate with the self-pipe trick via `pipe(2)`.
            //
            // For strict std-only: we spawn a thread that simply watches the
            // AtomicBool and the real SIGINT terminates child processes
            // (since children inherit terminal signals by default).
            //
            // The handler below is registered via `std::panic::set_hook`
            // + raw `signal` FFI call wrapped in a minimal unsafe block.
            register_unix_sigint_handler();
        }
    }

    #[cfg(windows)]
    {
        register_windows_ctrl_handler();
    }
}

#[cfg(unix)]
unsafe fn register_unix_sigint_handler() {
    // Raw FFI: install a signal handler that writes to a self-pipe.
    // We use a simpler approach: write 1 byte to a pipe in the signal handler,
    // and a background thread reads from the read end and sets CANCEL_TOKEN.
    //
    // Self-pipe trick avoids async-signal-safety issues.

    use std::os::unix::io::FromRawFd;

    extern "C" fn sigint_handler(_sig: libc_signum) {
        // Write a byte to the write end of the self-pipe.
        // SAFETY: write(2) is async-signal-safe.
        let _ = write_signal_byte();
        // Re-raise default to allow process to actually exit if needed.
    }

    // Create pipe
    let mut fds: [i32; 2] = [0; 2];
    if pipe_syscall(&mut fds) != 0 {
        return;
    }

    let read_fd = fds[0];
    let write_fd = fds[1];

    // Store write_fd globally for the signal handler.
    SIGNAL_PIPE_WRITE_FD.store(write_fd, std::sync::atomic::Ordering::Relaxed);

    // Install SIGINT handler
    install_sigaction(sigint_handler as usize);

    // Spawn background thread that reads the pipe and sets CANCEL_TOKEN.
    let _ = std::thread::Builder::new()
        .name("drakkar-sigint-watcher".to_string())
        .spawn(move || {
            let mut buf = [0u8; 1];
            loop {
                let n = read_from_fd(read_fd, &mut buf);
                if n > 0 {
                    eprintln!("\n\x1b[33mCancelling build (Ctrl+C)...\x1b[0m");
                    cancel();
                    // Close write end to let subsequent reads return 0 (EOF)
                    // so we don't spin, break after first signal.
                    break;
                } else {
                    break;
                }
            }
        });
}

// ---- Minimal Unix FFI (only used when compiling on Unix) ----
#[cfg(unix)]
type libc_signum = libc_int;
#[cfg(unix)]
type libc_int = std::ffi::c_int;

#[cfg(unix)]
static SIGNAL_PIPE_WRITE_FD: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(-1);

#[cfg(unix)]
fn write_signal_byte() -> isize {
    let fd = SIGNAL_PIPE_WRITE_FD.load(std::sync::atomic::Ordering::Relaxed);
    if fd < 0 {
        return -1;
    }
    let byte: u8 = 1;
    unsafe { libc_write(fd, &byte as *const u8 as *const std::ffi::c_void, 1) }
}

#[cfg(unix)]
fn pipe_syscall(fds: &mut [i32; 2]) -> i32 {
    unsafe { libc_pipe(fds.as_mut_ptr()) }
}

#[cfg(unix)]
fn read_from_fd(fd: i32, buf: &mut [u8]) -> isize {
    unsafe { libc_read(fd, buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len()) }
}

#[cfg(unix)]
fn install_sigaction(handler_addr: usize) {
    // Use raw syscall via inline assembly or extern "C" linkage.
    // This is the minimal FFI we permit.
    unsafe {
        let mut sa: libc_sigaction = std::mem::zeroed();
        sa.sa_handler = handler_addr;
        sa.sa_flags = SA_RESTART;
        libc_sigaction(SIGINT, &sa, std::ptr::null_mut());
    }
}

// Minimal libc FFI declarations for Unix signal handling.
// These are available on all Unix-like systems.
#[cfg(unix)]
extern "C" {
    fn pipe(fds: *mut libc_int) -> libc_int;
    fn read(fd: libc_int, buf: *mut std::ffi::c_void, count: usize) -> isize;
    fn write(fd: libc_int, buf: *const std::ffi::c_void, count: usize) -> isize;
    fn sigaction(
        signum: libc_int,
        act: *const libc_sigaction,
        oldact: *mut libc_sigaction,
    ) -> libc_int;
}

#[cfg(unix)]
unsafe fn libc_pipe(fds: *mut libc_int) -> libc_int {
    pipe(fds)
}

#[cfg(unix)]
unsafe fn libc_read(fd: libc_int, buf: *mut std::ffi::c_void, count: usize) -> isize {
    read(fd, buf, count)
}

#[cfg(unix)]
unsafe fn libc_write(fd: libc_int, buf: *const std::ffi::c_void, count: usize) -> isize {
    write(fd, buf, count)
}

#[cfg(unix)]
unsafe fn libc_sigaction(
    signum: libc_int,
    act: *const libc_sigaction,
    oldact: *mut libc_sigaction,
) -> libc_int {
    sigaction(signum, act, oldact)
}

// libc_sigaction struct (simplified for our purposes)
#[cfg(unix)]
#[repr(C)]
struct libc_sigaction {
    sa_handler: usize,
    sa_flags: i64,
    sa_restorer: usize,
    sa_mask: [u64; 16],
}

#[cfg(unix)]
const SIGINT: libc_int = 2;
#[cfg(unix)]
const SA_RESTART: i64 = 0x10000000;

// ---- Windows Ctrl+C handler (Variant A) ----
#[cfg(windows)]
fn register_windows_ctrl_handler() {
    extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            0 | 1 => {
                // CTRL_C_EVENT or CTRL_BREAK_EVENT
                eprintln!("\n\x1b[33mCancelling build (Ctrl+C)...\x1b[0m");
                cancel();
                1 // handled
            }
            _ => 0,
        }
    }

    extern "system" {
        fn SetConsoleCtrlHandler(handler: extern "system" fn(u32) -> i32, add: i32) -> i32;
    }

    unsafe {
        SetConsoleCtrlHandler(ctrl_handler, 1);
    }
}

/// Kill a child process group (Variant B, Unix only).
/// If `use_process_groups` is false or platform is not Unix, does nothing.
#[cfg(unix)]
pub fn kill_process_group(pgid: u32) {
    extern "C" {
        fn killpg(pgrp: libc_int, sig: libc_int) -> libc_int;
    }
    const SIGKILL: libc_int = 9;
    unsafe {
        killpg(pgid as libc_int, SIGKILL);
    }
}

#[cfg(not(unix))]
pub fn kill_process_group(_pgid: u32) {
    // No-op on non-Unix
}

/// Configure a Command to run in its own process group (Variant B, Unix only).
/// Returns the pgid to use for killing.
#[cfg(unix)]
pub fn set_process_group(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            // Create new process group with pgid == pid
            let ret = libc_setpgid(0, 0);
            if ret != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn libc_setpgid(pid: i32, pgid: i32) -> i32 {
    extern "C" {
        fn setpgid(pid: libc_int, pgid: libc_int) -> libc_int;
    }
    unsafe { setpgid(pid, pgid) }
}

#[cfg(not(unix))]
pub fn set_process_group(_command: &mut std::process::Command) {
    // No-op
}
