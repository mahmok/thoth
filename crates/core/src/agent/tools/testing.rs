use serde_json::Value;

use super::tool::{Context as ToolContext, Outcome as ToolResult, Tool};
use super::tool_set::ToolSet;

pub(super) async fn run<T>(tool: T, args: Value) -> ToolResult
where
    T: Tool,
{
    let name = tool.name();
    let mut set = ToolSet::new();
    set.register(tool).unwrap();
    set.run(name, &args.to_string(), &ToolContext::detached()).await
}

pub(super) async fn run_with_context<T>(tool: T, args: Value, cx: &ToolContext) -> ToolResult
where
    T: Tool,
{
    let name = tool.name();
    let mut set = ToolSet::new();
    set.register(tool).unwrap();
    set.run(name, &args.to_string(), cx).await
}
