//! Windows Job Object helpers for process-tree reaping.
//!
//! This crate is the **sanctioned unsafe boundary** for `lab`'s Windows Job
//! Object FFI, mirroring how the Unix path routes its unsafe through the
//! external `nix` crate. The workspace sets `unsafe_code = "forbid"` (which a
//! `#[allow]` cannot escape), so `lab` and `lab-apis` stay unsafe-free. The raw
//! `windows-sys` calls are encapsulated here behind a **safe** public API:
//! callers never write `unsafe`.
//!
//! On Windows there is no concept of a process group. A Job Object is the
//! nearest OS equivalent: the kernel associates a child process (and, when
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is set, its entire descendant tree)
//! with the job. Closing the last handle to the job with that flag set causes
//! the OS to terminate every process in the job — including descendants created
//! after the direct child is assigned to the job. Descendants born before the
//! direct child is assigned are not retroactively captured by Windows.
//!
//! ## Design choice: raw `windows-sys` + `AssignProcessToJobObject`
//!
//! We use `windows-sys` plus `AssignProcessToJobObject` after spawn (mirroring
//! how the Unix path uses raw `pgid` + `killpg`) rather than `process_wrap`'s
//! `JobObject` wrapper. Reason: `process_wrap`'s `JobObject` integrates with
//! its own `TokioChildProcess` drop semantics and sets `CREATE_SUSPENDED` so
//! the child does not race ahead before the job assignment. Once `rmcp`'s
//! `TokioChildProcess::builder(...).spawn()` takes ownership of the spawned
//! child process, the `process_wrap` drop-based cleanup no longer fires — that
//! is exactly why the Unix path uses a separate raw-`pgid` `ProcessGroupGuard`.
//! The Windows guard mirrors that shape: we obtain the child's HANDLE via
//! `OpenProcess`, assign it to a fresh job, set `KILL_ON_JOB_CLOSE`, then own
//! just the job handle in the guard. Drop closes the handle and the OS reaps
//! the whole tree.
//!
//! ## Spawn → assign race (accepted)
//!
//! Because we call `AssignProcessToJobObject` after the child has already been
//! spawned (not `CREATE_SUSPENDED` + resume), there is a window in which the
//! child can itself spawn grandchildren *before* the assignment completes. Any
//! grandchild born in that window will NOT be in the job and therefore will not
//! be reaped by `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
//!
//! This race is accepted for the following reasons:
//! - The window is extremely short (nanoseconds between `spawn()` returning and
//!   `OpenProcess` + `AssignProcessToJobObject` completing).
//! - Typical upstream MCP servers (`npx`, `uvx`, shell wrappers) do not spawn
//!   grandchildren synchronously in their first nanoseconds of execution.
//! - Using `CREATE_SUSPENDED` requires re-implementing the spawn path around
//!   `CreateProcess` directly, which is complex and would duplicate logic
//!   already in Tokio's process spawner.
//! - The Unix path has an analogous window between `spawn()` and the child's
//!   call to `setsid()`/`setpgid()` in `process_wrap::ProcessGroup::leader()`.
//!
//! If a future use-case requires a truly race-free assignment, the correct
//! approach is to use `JobObject` from `process-wrap` (which passes
//! `CREATE_SUSPENDED` to `CreateProcess`) with a custom Tokio child-process
//! wrapper that resumes the process after the job is assigned.
//!
//! ## Handle representation
//!
//! The job handle is stored and passed as `isize` (the raw `HANDLE` value), not
//! `HANDLE` itself. In `windows-sys 0.59` `HANDLE` is `*mut c_void`
//! (`!Send + !Sync`), which would poison the `Send`/`Sync` bounds of any struct
//! that stores it (and break `lab`'s axum router). `isize` is
//! `Copy + Send + Sync`; we cast back to `HANDLE` only at the `CloseHandle`
//! boundary inside [`close_job`].
//!
//! On non-Windows targets this crate compiles to an empty library (`lab` only
//! depends on it under `cfg(windows)`).

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
    },
};

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinJobError {
    pub operation: &'static str,
    pub code: u32,
}

#[cfg(windows)]
impl WinJobError {
    #[must_use]
    pub fn last(operation: &'static str) -> Self {
        Self {
            operation,
            code: unsafe { GetLastError() },
        }
    }
}

#[cfg(windows)]
impl std::fmt::Display for WinJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed with Win32 error {}",
            self.operation, self.code
        )
    }
}

#[cfg(windows)]
impl std::error::Error for WinJobError {}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessLiveness {
    Alive,
    Exited,
    NotFound,
}

/// Create a new Windows Job Object, assign the given PID to it, and set
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so every descendant is terminated
/// when the last handle to the job is closed.
///
/// Returns the job handle as an `isize` (the raw `HANDLE` value), or `0` on
/// any Win32 failure (logged as a warning but never fatal — the caller falls
/// back to per-PID kill).
///
/// The handle is returned as `isize` rather than `HANDLE` (`*mut c_void`)
/// deliberately: in `windows-sys 0.59` `HANDLE` is a raw pointer, which is
/// `!Send + !Sync`. Storing it raw in a struct would poison that struct's
/// `Send`/`Sync` bounds. `isize` is `Copy + Send + Sync`, so the stored/passed
/// value crosses thread boundaries cleanly; we cast back to `HANDLE` only at
/// the `CloseHandle` boundary.
///
/// This is a **safe** function: the `unsafe` FFI is fully encapsulated here, so
/// callers (in `lab`, which forbids unsafe) never write `unsafe`.
#[cfg(windows)]
#[must_use]
pub fn create_job_for_pid(pid: u32) -> isize {
    // Open the child process with the rights needed to assign it to a job and
    // to terminate it.
    let proc_handle = unsafe {
        OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE,
            0, // bInheritHandle = FALSE
            pid,
        )
    };
    if proc_handle.is_null() || proc_handle == INVALID_HANDLE_VALUE {
        let error = WinJobError::last("OpenProcess");
        tracing::warn!(
            target: "labby_winjob",
            pid,
            code = error.code,
            "OpenProcess failed — job object not created; falling back to per-PID kill"
        );
        return 0;
    }

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        let error = WinJobError::last("CreateJobObjectW");
        tracing::warn!(
            target: "labby_winjob",
            pid,
            code = error.code,
            "CreateJobObjectW failed — falling back to per-PID kill"
        );
        unsafe { CloseHandle(proc_handle) };
        return 0;
    }

    // Fresh job: set KILL_ON_JOB_CLOSE directly. A non-zero returned handle
    // must mean the kill-on-close contract is active.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let set_ok = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(info).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(u32::MAX),
        )
    };
    if set_ok == 0 {
        let error = WinJobError::last("SetInformationJobObject");
        tracing::warn!(
            target: "labby_winjob",
            code = error.code,
            pid,
            "SetInformationJobObject failed — KILL_ON_JOB_CLOSE not set; closing job"
        );
        unsafe {
            CloseHandle(proc_handle);
            CloseHandle(job);
        }
        return 0;
    }

    let assigned = unsafe { AssignProcessToJobObject(job, proc_handle) };
    unsafe { CloseHandle(proc_handle) };
    if assigned == 0 {
        let error = WinJobError::last("AssignProcessToJobObject");
        tracing::warn!(
            target: "labby_winjob",
            pid,
            code = error.code,
            "AssignProcessToJobObject failed — job created but child not assigned; closing job"
        );
        unsafe { CloseHandle(job) };
        return 0;
    }

    tracing::debug!(
        target: "labby_winjob",
        pid,
        "process assigned to job object with KILL_ON_JOB_CLOSE"
    );
    // Return the raw HANDLE value as isize — Send/Sync-safe to store and pass.
    job as isize
}

/// Close the job object handle, causing the OS to terminate all processes in
/// the job (including grandchildren) if `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
/// was set.
///
/// Takes the handle as `isize` (the `Send`/`Sync`-safe representation stored by
/// the caller); `0` is the "no job" sentinel and is a no-op. The value is cast
/// back to `HANDLE` only here, immediately before `CloseHandle`.
///
/// Logs warnings on failure but never panics — safe to call from `Drop`.
///
/// This is a **safe** function: the `unsafe` FFI is fully encapsulated here.
#[cfg(windows)]
pub fn close_job(job: isize, pid: u32) {
    if job == 0 {
        return;
    }
    let job = job as HANDLE;
    let ok = unsafe { CloseHandle(job) };
    if ok == 0 {
        tracing::warn!(
            target: "labby_winjob",
            pid,
            "CloseHandle(job) failed — descendant processes may have orphaned"
        );
    } else {
        tracing::debug!(
            target: "labby_winjob",
            pid,
            "job object handle closed — OS reaping descendant tree"
        );
    }
}

/// Return `true` if `pid` refers to a process that is still running.
///
/// Used by the Job Object reaping integration test (which lives in `lab`, where
/// unsafe is forbidden) to assert that a grandchild was terminated. The unsafe
/// FFI is encapsulated here so the test stays unsafe-free.
///
/// Returns `false` if the process has exited, was never present, or cannot be
/// opened (e.g. insufficient permission) — for the test's purposes "not
/// observably alive" is the signal it needs.
#[cfg(windows)]
#[must_use]
pub fn pid_is_alive(pid: u32) -> bool {
    matches!(pid_liveness(pid), Ok(ProcessLiveness::Alive))
}

#[cfg(windows)]
pub fn pid_liveness(pid: u32) -> Result<ProcessLiveness, WinJobError> {
    // `WAIT_OBJECT_0` is a `WAIT_EVENT` constant in `Win32::Foundation` (not
    // `Threading`) in windows-sys 0.59; `WaitForSingleObject` returns it.
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        let error = WinJobError::last("OpenProcess");
        if error.code == ERROR_INVALID_PARAMETER {
            return Ok(ProcessLiveness::NotFound);
        }
        return Err(error);
    }
    // Zero timeout: an already-exited process signals immediately (WAIT_OBJECT_0).
    let result = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    match result {
        WAIT_OBJECT_0 => Ok(ProcessLiveness::Exited),
        WAIT_TIMEOUT => Ok(ProcessLiveness::Alive),
        WAIT_FAILED => Err(WinJobError::last("WaitForSingleObject")),
        other => Err(WinJobError {
            operation: "WaitForSingleObject",
            code: other,
        }),
    }
}

/// Find the PID of the first child process whose parent is `parent_pid`.
///
/// Walks the system process snapshot via the ToolHelp API. Used by the Job
/// Object reaping integration test to locate the grandchild (`ping.exe` under
/// `cmd`). Returns `None` if the snapshot cannot be taken or no child is found.
///
/// The unsafe FFI is encapsulated here so the test (in `lab`) stays unsafe-free.
#[cfg(windows)]
#[must_use]
pub fn find_first_child_pid(parent_pid: u32) -> Option<u32> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    let Ok(size) = u32::try_from(size_of::<PROCESSENTRY32W>()) else {
        unsafe { CloseHandle(snap) };
        return None;
    };
    entry.dwSize = size;

    let mut found: Option<u32> = None;
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        if entry.th32ParentProcessID == parent_pid {
            found = Some(entry.th32ProcessID);
            break;
        }
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snap) };
    found
}
