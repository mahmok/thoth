use alloc::sync::Arc;
use core::fmt::Write as _;
use core::time::Duration;
use std::process::Stdio;
use std::sync::Mutex;
use tokio::time;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::adapters::workspace::Workspace;
use crate::domain::tool::Outcome as ToolResult;
use crate::runtime::tools::tool::{Description, ToolContext};
use crate::sync::MutexExt as _;

/// How much of each stream's start and end is kept. The start usually holds the first (and most
/// useful) error, the end holds the summary, and the middle is rarely needed.
const HEAD_BYTES: usize = 5_000;
/// How much of the end of the output is kept.
const TAIL_BYTES: usize = 10_000;
/// The timeout when a call gives none.
pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// The longest timeout a call may ask for.
pub(super) const MAX_TIMEOUT_SECS: u64 = 600;
/// How long to wait for the pipes to empty once the command has ended. A process it left running
/// in the background may hold them open for good.
const PIPE_GRACE: Duration = Duration::from_millis(300);

/// The shell a shell-based Tool runs its commands in.
pub(super) struct Shell {
    /// The executable to start.
    pub(super) program: &'static str,
    /// The flags that go before the command text.
    pub(super) options: &'static [&'static str],
    /// The shell's name, as the model and errors call it.
    pub(super) name: &'static str,
    /// What the model is told this Tool does.
    pub(super) description: &'static str,
}

/// Shared arguments of the shell-based Tools.
#[derive(Debug, Deserialize)]
pub(super) struct ShellArgs {
    pub(super) command: String,
    #[serde(default)]
    pub(super) timeout_secs: Option<u64>,
}

impl ShellArgs {
    /// The timeout this call asks for, clamped to the allowed range.
    pub(super) fn timeout(&self) -> Duration {
        let secs = self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS);
        Duration::from_secs(secs)
    }
}

/// The description every shell-based Tool gives the model.
pub(super) const fn describe(shell: &Shell) -> Description {
    Description { description: shell.description, name: shell.name }
}

/// The parameter schema every shell-based Tool shows the model.
pub(super) fn parameters(shell: &Shell) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": format!("The {} command to run.", shell.name) },
            "timeout_secs": {
                "type": "integer",
                "description": format!("Seconds before the command is killed (default {DEFAULT_TIMEOUT_SECS}, max {MAX_TIMEOUT_SECS}).")
            }
        },
        "required": ["command"]
    })
}

/// A shell Tool's `run`, shared by every platform's shell.
pub(super) async fn run(workspace: &Workspace, shell: &Shell, args: ShellArgs, _cx: &ToolContext) -> ToolResult {
    execute(workspace, shell, &args.command, args.timeout()).await
}

/// The start and the end of what a stream produced, however long it was.
#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl Capture {
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len());
        let room = HEAD_BYTES.saturating_sub(self.head.len()).min(bytes.len());
        let (for_head, for_tail) = bytes.split_at_checked(room).unwrap_or((bytes, &[]));
        self.head.extend_from_slice(for_head);
        self.tail.extend_from_slice(for_tail);
        if self.tail.len() > 2 * TAIL_BYTES {
            let excess = self.tail.len().saturating_sub(TAIL_BYTES);
            drop(self.tail.drain(..excess));
        }
    }

    fn text(&self) -> String {
        if self.total <= HEAD_BYTES + TAIL_BYTES {
            let all = [self.head.as_slice(), self.tail.as_slice()].concat();
            return String::from_utf8_lossy(&all).into_owned();
        }
        let tail = self.tail.get(self.tail.len().saturating_sub(TAIL_BYTES)..).unwrap_or_default();
        let omitted = self.total.saturating_sub(self.head.len()).saturating_sub(tail.len());
        format!(
            "{}\n[truncated: {omitted} bytes omitted from the middle]\n{}",
            String::from_utf8_lossy(&self.head),
            String::from_utf8_lossy(tail)
        )
    }
}

/// Output of a stream, shared with its reader.
type Shared = Arc<Mutex<Capture>>;

/// Kills the command's whole process group when it is dropped while armed: on a timeout, and when
/// the call is cancelled. Only the command itself is killed otherwise, and what it started would
/// run on. A command that ends by itself disarms it, so a server it left running stays.
struct GroupKill(Option<u32>);

impl GroupKill {
    const fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            kill_group(pid);
        }
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    let _unused =
        std::process::Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).stderr(Stdio::null()).status();
}

#[cfg(windows)]
fn kill_group(pid: u32) {
    let _unused = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
const fn kill_group(_pid: u32) {}

fn drain(mut reader: impl AsyncRead + Unpin + Send + 'static, into: Shared) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => into.lock_unpoisoned().push(buffer.get(..read).unwrap_or_default()),
            }
        }
    })
}

fn report(stdout: &Capture, stderr: &Capture) -> String {
    let (stdout_text, stderr_text) = (stdout.text(), stderr.text());
    let mut text = String::new();
    if !stdout_text.is_empty() {
        let _written = write!(text, "stdout:\n{}\n", stdout_text.trim_end());
    }
    if !stderr_text.is_empty() {
        let _written = write!(text, "stderr:\n{}\n", stderr_text.trim_end());
    }
    text.trim_end().to_owned()
}

/// Runs `command` in `shell` with a timeout, killing its process group when it ends or is cancelled.
async fn execute(workspace: &Workspace, shell: &Shell, command: &str, timeout: Duration) -> ToolResult {
    let mut process = Command::new(shell.program);
    // Its own process group, so that the whole group can be killed.
    #[cfg(unix)]
    let _group = process.process_group(0);
    let spawned = process
        .args(shell.options)
        .arg(command)
        .current_dir(workspace.root())
        // The output goes to a pipe, not a terminal: nothing should wait for a pager.
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return ToolResult::error(format!("cannot start {}: {e}", shell.name)),
    };
    let mut group = GroupKill(child.id());
    let (stdout, stderr) = (Shared::default(), Shared::default());
    let readers = [
        child.stdout.take().map(|pipe| drain(pipe, Arc::clone(&stdout))),
        child.stderr.take().map(|pipe| drain(pipe, Arc::clone(&stderr))),
    ];

    let waited = time::timeout(timeout, child.wait()).await;
    if waited.is_err() {
        drop(group);
        let _unused = child.kill().await;
    } else {
        group.disarm();
    }
    for reader in readers.into_iter().flatten() {
        let abort = reader.abort_handle();
        if time::timeout(PIPE_GRACE, reader).await.is_err() {
            abort.abort();
        }
    }
    let text = report(&stdout.lock_unpoisoned(), &stderr.lock_unpoisoned());

    let status = match waited {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return ToolResult::error(format!("command failed: {e}")),
        Err(_) => {
            let mut message = format!("command timed out after {}s and was killed", timeout.as_secs());
            if !text.is_empty() {
                let _written = write!(message, "; the output so far:\n{text}");
            }
            return ToolResult::error(message);
        },
    };
    let shown = if text.is_empty() { "(no output)".to_owned() } else { text };
    match status.code() {
        Some(0i32) => ToolResult::ok(shown),
        Some(code) => ToolResult { content: format!("exit code: {code}\n{shown}"), is_error: true, usage: None },
        None => ToolResult { content: format!("terminated by a signal\n{shown}"), is_error: true, usage: None },
    }
}
