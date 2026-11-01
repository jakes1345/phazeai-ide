//! Constructors for child processes that never open a console window, and
//! [`output_or_kill`] for commands that must not outlive the tool call that
//! started them.
//!
//! The desktop IDE is a GUI-subsystem program on Windows, so it has no console
//! of its own. Without `CREATE_NO_WINDOW`, every console program it starts
//! (git, ripgrep, language servers, debug adapters) would pop up a new,
//! empty console window for as long as it runs. On other platforms these are
//! plain `Command::new` calls.

use std::ffi::OsStr;
use std::process::{Child, Output, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// `CREATE_NO_WINDOW` from the Win32 process-creation flags.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `std::process::Command` for `program` that does not open a console window.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A `tokio::process::Command` for `program` that does not open a console window.
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Runs `cmd` to the end like [`tokio::process::Command::output`] (stdin
/// closed, stdout and stderr captured), or kills it: when `timeout` passes
/// first (`Ok(None)`), or when the returned future is dropped.
///
/// Dropping is what happens when the user interrupts a run (Stop in the IDE,
/// Esc / Ctrl+C in the CLI): the agent stops awaiting the tool. A dropped
/// `output()` future leaves its child running, and killing only the child is
/// not enough either: `bash -c "cargo build"` is bash plus cargo plus rustc,
/// and `cargo check` runs rustc and build scripts. So the command runs in a
/// process group of its own (Unix) and the whole group is killed. On Windows
/// only the direct child is killed, through the `Child` handle.
pub async fn output_or_kill(
    mut cmd: tokio::process::Command,
    timeout: Duration,
) -> std::io::Result<Option<Output>> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Dropping the child kills it…
        .kill_on_drop(true);
    // …and a process group of its own lets `KillGroupOnDrop` take whatever
    // it started along with it.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn()?;
    let mut guard = KillGroupOnDrop { pid: child.id() };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let finished = {
        let run = async { tokio::join!(child.wait(), read_to_end(stdout), read_to_end(stderr)) };
        tokio::select! {
            result = run => Some(result),
            _ = tokio::time::sleep(timeout) => None,
        }
    };
    let Some((status, stdout, stderr)) = finished else {
        kill_and_reap(&mut child).await;
        guard.disarm();
        return Ok(None);
    };
    guard.disarm();
    Ok(Some(Output {
        status: status?,
        stdout,
        stderr,
    }))
}

/// Kills the process group led by `pid`: the command and everything it
/// started. On Windows the direct child is killed through the `Child` handle
/// instead.
fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: plain libc call. A negative pid addresses the process group
        // with that id, which is the command's own group because it was
        // spawned with `process_group(0)`. Failure (already gone) is harmless.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Kills the command's process group if it is still running when the
/// [`output_or_kill`] future is dropped. Disarmed once the command has exited.
struct KillGroupOnDrop {
    pid: Option<u32>,
}

impl KillGroupOnDrop {
    fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kill_process_group(pid);
        }
    }
}

/// Kills a child that is still running (timeout) and reaps it.
async fn kill_and_reap(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        kill_process_group(pid);
    }
    // On Windows this is the kill itself; on Unix the group is already dead
    // and this reaps the leader.
    let _ = child.kill().await;
}

async fn read_to_end(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf).await;
    }
    buf
}

/// A server process that two parties may end: the client that started it
/// (on shutdown, disconnect or drop) and the thread reading its output (when
/// that output ends because the process exited or crashed).
///
/// Whoever ends it first kills it and collects its exit status; later calls
/// find the slot empty. Without the collecting `wait`, a process that has
/// ended stays in the process table as a zombie until PhazeAI exits, one more
/// for every restart of a crashing server. After a kill the wait returns at
/// once.
#[derive(Clone)]
pub(crate) struct ChildSlot(Arc<Mutex<Option<Child>>>);

impl ChildSlot {
    pub(crate) fn new(child: Child) -> Self {
        Self(Arc::new(Mutex::new(Some(child))))
    }

    /// The pid, while the process has not been ended (afterwards the pid may
    /// already belong to another process).
    pub(crate) fn id(&self) -> Option<u32> {
        self.lock().as_ref().map(Child::id)
    }

    /// Kill the process if it still runs and reap it. Idempotent.
    pub(crate) fn end(&self) {
        // Taken out first so the lock is not held while waiting.
        let child = self.lock().take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_command_keeps_program() {
        assert_eq!(std_command("git").get_program(), "git");
    }

    #[test]
    fn tokio_command_keeps_program() {
        assert_eq!(tokio_command("git").as_std().get_program(), "git");
    }
}
