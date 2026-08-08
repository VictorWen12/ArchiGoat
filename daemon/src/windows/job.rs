//! Windows Job ownership receives one suspended child and hands back a fully reaped process tree.

use std::{ffi::c_void, mem::size_of, os::windows::process::CommandExt, time::Duration};
use tokio::process::{Child, Command};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_SET_QUOTA,
            PROCESS_TERMINATE, ResumeThread, THREAD_SUSPEND_RESUME,
        },
    },
};

/// Process-tree cleanup backs off while Windows drains descendants, avoiding a hot cleanup loop.
const MAX_REAP_WAIT: Duration = Duration::from_secs(1);
const INITIAL_REAP_WAIT: Duration = Duration::from_millis(25);
/// A broken Windows process report cannot leave finished Work waiting forever.
const REAP_DEADLINE: Duration = Duration::from_secs(5);

#[link(name = "kernel32")]
unsafe extern "system" {
    /// Creates the Windows container that owns one Work process tree.
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> HANDLE;
}

/// Keeps every process spawned by one Work under one cleanup owner.
pub(super) struct Job {
    handle: usize,
}

// This Windows job owns a Work's process tree for reliable isolated cleanup.
impl Job {
    /// Creates an empty process container that closes children on release.
    pub(super) fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if invalid(handle) {
            return Err(os_error("Could not create Work process ownership"));
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe { CloseHandle(handle) };
            return Err(os_error("Could not protect the Work process tree"));
        }
        Ok(Self {
            handle: handle as usize,
        })
    }

    /// Adds the suspended Provider process before it can create children.
    pub(super) fn assign(&self, child: &Child) -> Result<(), String> {
        let pid = child
            .id()
            .ok_or_else(|| "Work process identity is unavailable".to_owned())?;
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if invalid(process) {
            return Err(os_error("Could not open the Work process"));
        }
        let assigned = unsafe { AssignProcessToJobObject(self.raw(), process) };
        unsafe { CloseHandle(process) };
        (assigned != 0)
            .then_some(())
            .ok_or_else(|| os_error("Could not own the Work process tree"))
    }

    /// Stops and reaps every process owned by this Work.
    /// PHYSICS: reached only once the turn has really ended — the Provider exited, the owner pressed
    /// Stop, or the launch never took ownership. It ends processes, never a turn.
    pub(super) async fn finish(&self, child: &mut Child) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Err(error) = self.stop_tree() {
            errors.push(error);
            if let Err(error) = child.start_kill() {
                errors.push(format!("Could not signal the Work process: {error}"));
            }
        }
        if let Err(error) = child.wait().await {
            errors.push(format!("Could not reap the Work process: {error}"));
        }
        if let Err(error) = self.wait_empty().await {
            errors.push(error);
        }
        joined(errors)
    }

    /// Reaps the Work tree when async cleanup is unavailable.
    /// PHYSICS: the same already-ended turn as finish, on the path with no runtime to wait on.
    pub(super) fn finish_blocking(&self, child: &mut Child) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Err(error) = self.stop_tree() {
            errors.push(error);
            if let Err(error) = child.start_kill() {
                errors.push(format!("Could not signal the Work process: {error}"));
            }
        }
        let mut retry = INITIAL_REAP_WAIT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    std::thread::sleep(retry);
                    retry = (retry * 2).min(MAX_REAP_WAIT);
                }
                Err(error) => {
                    errors.push(format!("Could not reap the Work process: {error}"));
                    break;
                }
            }
        }
        if let Err(error) = self.wait_empty_blocking() {
            errors.push(error);
        }
        joined(errors)
    }

    /// Requests immediate termination of this Work tree.
    fn terminate(&self) -> Result<(), String> {
        (unsafe { TerminateJobObject(self.raw(), 1) } != 0)
            .then_some(())
            .ok_or_else(|| os_error("Could not terminate the Work process tree"))
    }

    /// Stops this Job without touching another Work.
    fn stop_tree(&self) -> Result<(), String> {
        match self.active() {
            Ok(0) => Ok(()),
            Ok(_) | Err(_) => self.terminate(),
        }
    }

    /// Waits until Windows reports that every child has exited.
    /// PHYSICS: bounds one cleanup confirmation after the turn ended; its failure only adds a clause
    /// to an outcome already decided.
    async fn wait_empty(&self) -> Result<(), String> {
        tokio::time::timeout(REAP_DEADLINE, async {
            let mut retry = INITIAL_REAP_WAIT;
            while self.active()? != 0 {
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(MAX_REAP_WAIT);
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|_| "Could not confirm Work process cleanup".to_owned())?
    }

    /// Waits synchronously until the Work owns no processes.
    fn wait_empty_blocking(&self) -> Result<(), String> {
        let started = std::time::Instant::now();
        let mut retry = INITIAL_REAP_WAIT;
        while self.active()? != 0 {
            if started.elapsed() >= REAP_DEADLINE {
                return Err("Could not confirm Work process cleanup".to_owned());
            }
            std::thread::sleep(retry);
            retry = (retry * 2).min(MAX_REAP_WAIT);
        }
        Ok(())
    }

    /// Counts processes still owned by this Work.
    fn active(&self) -> Result<u32, String> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast::<c_void>(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        (queried != 0)
            .then_some(info.ActiveProcesses)
            .ok_or_else(|| os_error("Could not verify Work process cleanup"))
    }

    /// Returns the Windows handle used by process assignment.
    fn raw(&self) -> HANDLE {
        self.handle as HANDLE
    }
}

// This cleanup releases the native job handle after its Work has ended.
impl Drop for Job {
    /// PHYSICS: closing a kill-on-close Job is the OS-owned last-resort process-tree cleanup, and it
    /// runs only once this Job's Work is gone from this process.
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.raw());
        }
    }
}

/// Starts a short background command paused for safe ownership setup.
pub(super) fn hidden_suspended(command: &mut Command) {
    command
        .as_std_mut()
        .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

/// Starts the Provider inside its existing hidden Work host, initially paused.
pub(super) fn console_suspended(command: &mut Command) {
    command.as_std_mut().creation_flags(CREATE_SUSPENDED);
}

/// Lets an owned suspended process begin execution.
pub(super) fn resume(child: &Child) -> Result<(), String> {
    let pid = child
        .id()
        .ok_or_else(|| "Suspended process identity is unavailable".to_owned())?;
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if invalid(snapshot) {
        return Err(os_error("Could not inspect the suspended process"));
    }
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..THREADENTRY32::default()
    };
    let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    let mut resumed = false;
    while found {
        if entry.th32OwnerProcessID == pid {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !invalid(thread) {
                resumed = unsafe { ResumeThread(thread) } != u32::MAX;
                unsafe { CloseHandle(thread) };
                if resumed {
                    break;
                }
            }
        }
        found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    resumed
        .then_some(())
        .ok_or_else(|| os_error("Could not start the owned process"))
}

/// Reaps a child when Job assignment could not complete.
/// PHYSICS: ownership was never established, so nothing can observe this process.
pub(super) async fn reap_unowned(child: &mut Child) -> Result<(), String> {
    let signal = child
        .start_kill()
        .map_err(|error| format!("Could not signal the unowned process: {error}"));
    let reaped = child
        .wait()
        .await
        .map(|_| ())
        .map_err(|error| format!("Could not reap the unowned process: {error}"));
    match (signal, reaped) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(signal), Err(reap)) => Err(format!("{signal}; {reap}")),
    }
}

/// Reports an invalid Windows handle as an actionable error.
fn invalid(handle: HANDLE) -> bool {
    handle.is_null() || handle == INVALID_HANDLE_VALUE
}

/// Adds the current Windows error to one operation.
fn os_error(action: &str) -> String {
    format!("{action}: {}", std::io::Error::last_os_error())
}

/// Preserves both the primary error and any cleanup error.
fn joined(errors: Vec<String>) -> Result<(), String> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
