use serde_json::Value;

use super::shell::{self, Shell, ShellArgs};
use super::tool::{Description, Outcome as ToolResult, Tool, ToolContext};
use crate::workspace::Workspace;

/// The shell the `bash` Tool runs commands in.
const BASH: Shell = Shell {
    program: "bash",
    options: &["-c"],
    name: "bash",
    description: "Run a bash command in the working directory and return its exit code, stdout and stderr. \
        Commands run non-interactively; long output keeps its start and its end, and slow commands are killed after the timeout \
        (the output so far is still returned). Do not use it to write files: use write_file and edit_file.",
};

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

impl Tool for Bash {
    type Args = ShellArgs;

    fn describe(&self) -> Description {
        shell::describe(&BASH)
    }

    fn parameters(&self) -> Value {
        shell::parameters(&BASH)
    }

    async fn run(&self, args: ShellArgs, cx: &ToolContext) -> ToolResult {
        shell::run(&self.workspace, &BASH, args, cx).await
    }

    fn summary(&self, args: &ShellArgs) -> String {
        args.command.clone()
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tools::testing::run;
    use crate::test::temp_tree::TempTree;
    use core::time::Duration;
    use serde_json::json;
    use std::process;
    use std::process::Stdio;
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
