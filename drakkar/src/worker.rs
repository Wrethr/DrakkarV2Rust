/// Parallel worker pool for concurrent compilation.
///
/// Uses `std::sync::mpsc` + `std::thread` — no external crates.
///
/// Design:
/// - N worker threads receive tasks over a channel.
/// - Each worker checks the global cancel token before/after each task.
/// - Results are returned over a separate channel.
/// - On FailFast: the first compile error causes immediate cancellation of all workers.
/// - On aggregate mode: all errors are collected and returned together.
///
/// Child process tracking:
/// - Each child process pid is registered in `ActiveChildren` (Arc<Mutex<HashSet>>).
/// - On cancellation, the main thread kills all active children.

use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::collections::HashSet;
use std::process::Command;

use crate::build::{ObjectFile, compile_source_to_object};
use crate::config::{ProjectConfig, BuildProfile};
use crate::error::BuildError;
use crate::platform::{is_cancelled, cancel};

// ─────────────────────────────────────────────
// ActiveChildren — process pid registry
// ─────────────────────────────────────────────

/// Tracks all active compiler child process PIDs so they can be killed on cancellation.
#[derive(Clone)]
pub struct ActiveChildren {
    inner: Arc<Mutex<HashSet<u32>>>,
}

impl ActiveChildren {
    pub fn new() -> Self {
        ActiveChildren {
            inner: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn add(&self, pid: u32) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(pid);
        }
    }

    pub fn remove(&self, pid: u32) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&pid);
        }
    }

    /// Kill all tracked children (best-effort, ignores errors).
    pub fn kill_all(&self) {
        if let Ok(guard) = self.inner.lock() {
            for &pid in guard.iter() {
                kill_pid(pid);
            }
        }
    }
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe {
            kill(pid as i32, 9); // SIGKILL
        }
    }

    #[cfg(windows)]
    {
        // Use TerminateProcess via OpenProcess
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn TerminateProcess(handle: *mut std::ffi::c_void, code: u32) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        const PROCESS_TERMINATE: u32 = 0x0001;
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}

// ─────────────────────────────────────────────
// Worker pool
// ─────────────────────────────────────────────

pub struct WorkerPool {
    config: Arc<ProjectConfig>,
    profile: BuildProfile,
    extra_flags: Arc<Vec<String>>,
    verbose: bool,
    aggregate: bool,
    active_children: ActiveChildren,
}

impl WorkerPool {
    pub fn new(
        config: Arc<ProjectConfig>,
        profile: BuildProfile,
        extra_flags: Vec<String>,
        verbose: bool,
        aggregate: bool,
    ) -> Self {
        WorkerPool {
            config,
            profile,
            extra_flags: Arc::new(extra_flags),
            verbose,
            aggregate,
            active_children: ActiveChildren::new(),
        }
    }

    /// Compile all objects in parallel. Returns all ObjectFiles (for linking)
    /// and either Ok(compiled_count) or Err on failure.
    pub fn run(&self, objects: Vec<ObjectFile>) -> Result<(Vec<ObjectFile>, usize), BuildError> {
        let num_workers = self.config.parallel_jobs.max(1);
        let total = objects.len();

        // Divide into: needs recompile vs already up-to-date
        let mut to_compile: Vec<ObjectFile> = Vec::new();
        let mut up_to_date: Vec<ObjectFile> = Vec::new();

        for obj in objects {
            if crate::build::should_recompile(&obj, &self.config) {
                to_compile.push(obj);
            } else {
                up_to_date.push(obj);
            }
        }

        let compile_count = to_compile.len();

        if compile_count == 0 {
            // All up-to-date
            let mut all = up_to_date;
            all.extend(std::iter::empty::<ObjectFile>()); // satisfy type
            return Ok((all, 0));
        }

        let total_to_compile = compile_count;
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Task channel: sender sends ObjectFile tasks to workers
        let (task_tx, task_rx) = mpsc::channel::<ObjectFile>();
        let task_rx = Arc::new(Mutex::new(task_rx));

        // Result channel: workers send results back
        let (res_tx, res_rx) = mpsc::channel::<Result<ObjectFile, BuildError>>();

        // Spawn workers
        let mut handles = Vec::new();
        for _ in 0..num_workers.min(compile_count) {
            let task_rx = Arc::clone(&task_rx);
            let res_tx = res_tx.clone();
            let config = Arc::clone(&self.config);
            let profile = self.profile.clone();
            let extra_flags = Arc::clone(&self.extra_flags);
            let verbose = self.verbose;
            let active_children = self.active_children.clone();
            let counter = Arc::clone(&counter);
            let total_to_compile = total_to_compile;

            let handle = thread::spawn(move || {
                loop {
                    // Check cancellation
                    if is_cancelled() {
                        break;
                    }

                    // Try to get a task
                    let obj = {
                        let rx = task_rx.lock().unwrap();
                        match rx.recv() {
                            Ok(o) => o,
                            Err(_) => break, // Channel closed
                        }
                    };

                    if is_cancelled() {
                        break;
                    }

                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    println!(
                        "\x1b[36mCompiling\x1b[0m [{}/{}] {}",
                        n,
                        total_to_compile,
                        obj.src.rel_path.display()
                    );

                    let result = compile_source_to_object(
                        &obj,
                        &config,
                        &profile,
                        &extra_flags,
                        verbose,
                        &active_children,
                    );

                    match result {
                        Ok(()) => {
                            let _ = res_tx.send(Ok(obj));
                        }
                        Err(e) => {
                            let _ = res_tx.send(Err(e));
                        }
                    }
                }
            });
            handles.push(handle);
        }

        // Send all tasks
        for obj in to_compile {
            if task_tx.send(obj).is_err() {
                break;
            }
        }
        drop(task_tx); // Signal workers: no more tasks

        // Collect results
        let mut errors: Vec<BuildError> = Vec::new();
        let mut compiled_objects: Vec<ObjectFile> = Vec::new();
        let mut received = 0;

        while received < compile_count {
            match res_rx.recv() {
                Ok(Ok(obj)) => {
                    compiled_objects.push(obj);
                    received += 1;
                }
                Ok(Err(e)) => {
                    received += 1;
                    if !self.aggregate {
                        // Fail-fast: cancel all workers and kill children
                        cancel();
                        self.active_children.kill_all();
                        errors.push(e);
                        break;
                    } else {
                        errors.push(e);
                    }
                }
                Err(_) => {
                    // All senders dropped (workers panicked or done)
                    break;
                }
            }
        }

        // Wait for all worker threads to finish
        for h in handles {
            let _ = h.join();
        }

        if is_cancelled() && errors.is_empty() {
            return Err(BuildError::Cancelled);
        }

        if !errors.is_empty() {
            if errors.len() == 1 {
                return Err(errors.remove(0));
            } else {
                return Err(BuildError::MultipleErrors(errors));
            }
        }

        // Combine compiled + up-to-date
        let mut all_objects = compiled_objects;
        all_objects.extend(up_to_date);

        Ok((all_objects, compile_count))
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_children_add_remove() {
        let ac = ActiveChildren::new();
        ac.add(1234);
        ac.add(5678);
        {
            let guard = ac.inner.lock().unwrap();
            assert!(guard.contains(&1234));
            assert!(guard.contains(&5678));
        }
        ac.remove(1234);
        {
            let guard = ac.inner.lock().unwrap();
            assert!(!guard.contains(&1234));
            assert!(guard.contains(&5678));
        }
    }
}
