use serde_json::Value;

use super::shell::{self, Shell, ShellArgs};
use crate::adapters::workspace::Workspace;
use crate::runtime::tools::tool::{Description, Outcome as ToolResult, Tool, ToolContext};

/// The shell the `powershell` Tool runs commands in.
const POWERSHELL: Shell = Shell {
    program: "powershell",
    options: &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"],
    name: "powershell",
    description: "Run a Windows PowerShell command in the working directory and return its exit code, stdout and stderr. \
        Commands run non-interactively; long output keeps its start and its end, and slow commands are killed after the timeout \
        (the output so far is still returned). Do not use it to write files: use write_file and edit_file.",
};

#[derive(Debug)]
/// The `powershell` Tool.
pub(super) struct PowerShell {
    workspace: Workspace,
}

impl PowerShell {
    pub(super) const fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

impl Tool for PowerShell {
    type Args = ShellArgs;

    fn describe(&self) -> Description {
        shell::describe(&POWERSHELL)
    }

    fn parameters(&self) -> Value {
        shell::parameters(&POWERSHELL)
    }

    async fn run(&self, args: ShellArgs, cx: &ToolContext) -> ToolResult {
        shell::run(&self.workspace, &POWERSHELL, args, cx).await
    }

    fn summary(&self, args: &ShellArgs) -> String {
        args.command.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::tools::testing::run;
    use crate::test::temp_tree::TempTree;
    use core::time::Duration;
    use serde_json::json;
    use std::process::Command;
    use std::time::Instant;
    use tokio::time;

    fn tool(tree: &TempTree) -> PowerShell {
        PowerShell::new(tree.workspace())
    }

    #[tokio::test]
    async fn runs_in_the_workspace_root() {
        let tree = TempTree::new("powershell-cwd").file("marker.txt", "hi");
        let result = run(tool(&tree), json!({ "command": "Get-ChildItem -Name" })).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("marker.txt"), "{}", result.content);
    }

    #[tokio::test]
    async fn failures_report_exit_code_and_stderr() {
        let tree = TempTree::new("powershell-fail");
        let result = run(tool(&tree), json!({ "command": "Write-Error oops; exit 3" })).await;
        assert!(result.is_error, "{}", result.content);
        assert!(result.content.contains("exit code: 3") && result.content.contains("oops"), "{}", result.content);
    }

    #[tokio::test]
    async fn slow_commands_are_killed() {
        let tree = TempTree::new("powershell-timeout");
        let result = run(tool(&tree), json!({ "command": "Start-Sleep -Seconds 5", "timeout_secs": 1i32 })).await;
        assert!(result.is_error);
        assert!(result.content.contains("timed out"), "{}", result.content);
    }

    #[tokio::test]
    async fn long_output_keeps_the_tail() {
        let tree = TempTree::new("powershell-long");
        let result = run(tool(&tree), json!({ "command": "'x' * 100000; 'END'" })).await;
        assert!(result.content.contains("[truncated") && result.content.ends_with("END"), "{}", result.content.len());
    }

    #[tokio::test]
    async fn silent_success_says_so() {
        let tree = TempTree::new("powershell-quiet");
        let result = run(tool(&tree), json!({ "command": "exit 0" })).await;
        assert_eq!(result, ToolResult::ok("(no output)"));
    }

    #[tokio::test]
    async fn long_output_keeps_the_start_and_the_end() {
        let tree = TempTree::new("powershell-head-tail");
        let result = run(tool(&tree), json!({ "command": "'FIRST'; 'x' * 100000; 'LAST'" })).await;
        let text = result.content;
        assert!(text.starts_with("stdout:\nFIRST"), "{}", text.get(..40).unwrap_or(&text));
        assert!(text.ends_with("LAST"), "{}", text.get(text.len().saturating_sub(40)..).unwrap_or(&text));
        assert!(text.contains("bytes omitted from the middle"));
        assert!(text.len() < 16_000, "{}", text.len());
    }

    #[tokio::test]
    async fn a_timeout_still_returns_what_the_command_printed() {
        let tree = TempTree::new("powershell-partial");
        let command = "[Console]::WriteLine('started'); [Console]::Out.Flush(); Start-Sleep -Seconds 30";
        let result = run(tool(&tree), json!({ "command": command, "timeout_secs": 5i32 })).await;
        assert!(result.is_error);
        assert!(result.content.contains("timed out") && result.content.contains("started"), "{}", result.content);
    }

    #[tokio::test]
    async fn a_background_process_holding_the_pipe_does_not_hold_up_the_result() {
        let tree = TempTree::new("powershell-background");
        let started = Instant::now();
        let command = "cmd.exe /c 'start /b ping -n 8 127.0.0.1 & echo done'";
        let result = run(tool(&tree), json!({ "command": command, "timeout_secs": 30i32 })).await;
        assert!(result.content.contains("done"), "{}", result.content);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[tokio::test]
    async fn a_timeout_kills_what_the_command_started_too() {
        let tree = TempTree::new("powershell-group");
        let command = "$p = Start-Process -FilePath cmd.exe -ArgumentList '/c','ping -n 30 127.0.0.1' -NoNewWindow \
                       -PassThru; [Console]::WriteLine($p.Id); [Console]::Out.Flush(); Start-Sleep -Seconds 30";
        let result = run(tool(&tree), json!({ "command": command, "timeout_secs": 5i32 })).await;
        let pid: u32 = result
            .content
            .lines()
            .find_map(|line| line.trim().parse().ok())
            .unwrap_or_else(|| panic!("no pid in {}", result.content));
        for _ in 0i32..40i32 {
            if !alive(pid) {
                return;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
        panic!("process {pid} is still running");
    }

    /// Whether a process with `pid` is running.
    fn alive(pid: u32) -> bool {
        let query = format!("if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}");
        Command::new("powershell")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &query])
            .status()
            .is_ok_and(|status| status.success())
    }
}
