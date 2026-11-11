use alloc::sync::Arc;
use core::fmt::Write as _;
use core::time::Duration;
use std::process;
use std::process::Stdio;
use std::sync::Mutex;
use tokio::time;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;
use tokio::task::JoinHandle;

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool, Traits as ToolTraits};
use crate::sync::MutexExt as _;
use crate::workspace::Workspace;

/// How much of each stream's start and end is kept. The start usually holds the first (and most
/// useful) error, the end holds the summary, and the middle is rarely needed.
const HEAD_BYTES: usize = 5_000;
/// How much of the end of the output is kept.
const TAIL_BYTES: usize = 10_000;
/// The timeout when a call gives none.
const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// The longest timeout a call may ask for.
const MAX_TIMEOUT_SECS: u64 = 600;
/// How long to wait for the pipes to empty once the command has ended. A process it left running
/// in the background may hold them open for good.
const PIPE_GRACE: Duration = Duration::from_millis(300);
#[derive(Debug)]

/// The `bash` Tool.
pub(super) struct Bash {
    workspace: Workspace,
}

impl Bash {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[derive(Debug, Deserialize)]
/// Arguments of `bash`.
pub(super) struct BashArgs {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl Tool for Bash {
    type Args = BashArgs;

    fn description(&self) -> &'static str {
        "Run a bash command in the working directory and return its exit code, stdout and stderr. \
         Commands run non-interactively; long output keeps its start and its end, and slow commands are killed after the timeout \
         (the output so far is still returned). Do not use it to write files: use write_file and edit_file."
    }

    fn name(&self) -> &'static str {
        "bash"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The bash command to run." },
                "timeout_secs": {
                    "type": "integer",
                    "description": format!("Seconds before the command is killed (default {DEFAULT_TIMEOUT_SECS}, max {MAX_TIMEOUT_SECS}).")
                }
            },
            "required": ["command"]
        })
    }

    async fn run(&self, args: BashArgs, _cx: &ToolContext) -> ToolResult {
        let secs = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS);
        execute(&self.workspace, &args.command, Duration::from_secs(secs)).await
    }

    fn summary(&self, args: &BashArgs) -> String {
        args.command.clone()
    }

    #[inline]
    fn traits(&self) -> ToolTraits {
        ToolTraits::default()
    }

    #[inline]
    fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
        self.traits()
    }
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

    fn fire(&mut self) {
        if let Some(pid) = self.0.take() {
            kill_group(pid);
        }
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        self.fire();
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    let _unused =
        process::Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).stderr(Stdio::null()).status();
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

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

/// Runs `command` in bash with a timeout, killing its process group when it ends or is cancelled.
async fn execute(workspace: &Workspace, command: &str, timeout: Duration) -> ToolResult {
    let mut process = Command::new("bash");
    // Its own process group, so that the whole group can be killed.
    #[cfg(unix)]
    let _group = process.process_group(0);
    let spawned = process
        .arg("-c")
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
        Err(e) => return ToolResult::error(format!("cannot start bash: {e}")),
    };
    let mut group = GroupKill(child.id());
    let (stdout, stderr) = (Shared::default(), Shared::default());
    let readers = [
        child.stdout.take().map(|pipe| drain(pipe, Arc::clone(&stdout))),
        child.stderr.take().map(|pipe| drain(pipe, Arc::clone(&stderr))),
    ];

    let waited = time::timeout(timeout, child.wait()).await;
    if waited.is_err() {
        group.fire();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::testing::run;
    use crate::test::temp_tree::TempTree;
    use std::process;
    use std::time::Instant;
    use tokio::time;

    fn tool(tree: &TempTree) -> Bash {
        Bash::new(tree.workspace())
    }

    #[tokio::test]
    async fn runs_in_the_workspace_root() {
        let tree = TempTree::new("bash-cwd").file("marker.txt", "hi");
        let result = run(tool(&tree), json!({ "command": "ls" })).await;
        assert!(!result.is_error);
        assert!(result.content.contains("marker.txt"), "{}", result.content);
    }

    #[tokio::test]
    async fn failures_report_exit_code_and_stderr() {
        let tree = TempTree::new("bash-fail");
        let result = run(tool(&tree), json!({ "command": "echo oops >&2; exit 3" })).await;
        assert!(result.is_error);
        assert!(result.content.contains("exit code: 3") && result.content.contains("oops"));
    }

    #[tokio::test]
    async fn slow_commands_are_killed() {
        let tree = TempTree::new("bash-timeout");
        let result = run(tool(&tree), json!({ "command": "sleep 5", "timeout_secs": 1i32 })).await;
        assert!(result.is_error);
        assert!(result.content.contains("timed out"));
    }

    #[tokio::test]
    async fn long_output_keeps_the_tail() {
        let tree = TempTree::new("bash-long");
        let result = run(tool(&tree), json!({ "command": "yes x | head -c 100000; echo END" })).await;
        assert!(result.content.contains("[truncated") && result.content.ends_with("END"));
    }

    #[tokio::test]
    async fn silent_success_says_so() {
        let tree = TempTree::new("bash-quiet");
        let result = run(tool(&tree), json!({ "command": "true" })).await;
        assert_eq!(result, ToolResult::ok("(no output)"));
    }

    #[tokio::test]
    async fn long_output_keeps_the_start_and_the_end() {
        let tree = TempTree::new("bash-head-tail");
        let result =
            run(tool(&tree), json!({ "command": "echo FIRST; yes x | head -c 100000; echo; echo LAST" })).await;
        let text = result.content;
        assert!(text.starts_with("stdout:\nFIRST"), "{}", text.get(..40).unwrap_or(&text));
        assert!(text.ends_with("LAST"));
        assert!(text.contains("bytes omitted from the middle"));
        assert!(text.len() < 16_000, "{}", text.len());
    }

    #[tokio::test]
    async fn a_timeout_still_returns_what_the_command_printed() {
        let tree = TempTree::new("bash-partial");
        let result = run(tool(&tree), json!({ "command": "echo started; sleep 5", "timeout_secs": 1i32 })).await;
        assert!(result.is_error);
        assert!(result.content.contains("timed out") && result.content.contains("started"), "{}", result.content);
    }

    #[tokio::test]
    async fn a_background_process_holding_the_pipe_does_not_hold_up_the_result() {
        let tree = TempTree::new("bash-background");
        let started = Instant::now();
        let result = run(tool(&tree), json!({ "command": "sleep 3 & echo done", "timeout_secs": 30i32 })).await;
        assert!(result.content.contains("done"), "{}", result.content);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_kills_what_the_command_started_too() {
        let tree = TempTree::new("bash-group");
        let result = run(tool(&tree), json!({ "command": "sleep 30 & echo $!; sleep 30", "timeout_secs": 1i32 })).await;
        let pid: u32 = result
            .content
            .lines()
            .find_map(|line| line.trim().parse().ok())
            .unwrap_or_else(|| panic!("no pid in {}", result.content));
        let alive = || {
            process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        for _ in 0i32..40i32 {
            if !alive() {
                return;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
        panic!("process {pid} is still running");
    }
}
