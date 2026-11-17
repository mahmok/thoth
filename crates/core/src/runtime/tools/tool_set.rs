use alloc::fmt;
use core::error;

use crate::adapters::llm::client::ToolSpec;

use super::tool::{ErasedTool, Tool, ToolContext, Traits as ToolTraits};
use crate::domain::tool::Outcome as ToolResult;

#[derive(Debug, PartialEq, Eq)]
/// Returned when a Tool name is registered twice.
pub struct DuplicateTool(pub String);

impl fmt::Display for DuplicateTool {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a tool named `{}` is already registered", self.0)
    }
}

impl error::Error for DuplicateTool {}

#[derive(Debug)]
/// One registered Tool: what the model is told about it, and the Tool itself.
struct Entry {
    spec: ToolSpec,
    tool: Box<dyn ErasedTool>,
}

#[derive(Default, Debug)]
/// The Tools an Agent can call.
pub struct ToolSet {
    //TODO: Maybe convert to hashmap or tree.
    entries: Vec<Entry>,
}

impl ToolSet {
    #[inline]
    /// The definitions the model is sent, in registration order.
    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &ToolSpec> + '_ {
        self.entries.iter().map(|entry| &entry.spec)
    }

    #[must_use]
    #[inline]
    /// A short description of a call, for showing it.
    pub fn describe(&self, name: &str, raw_arguments: &str) -> String {
        self.find(name).map_or_else(
            || format!("{name} (unknown tool)"),
            |tool| format!("{name}({})", tool.describe_call(raw_arguments)),
        )
    }

    #[must_use]
    #[inline]
    /// The rough tokens the definitions cost in every request. Recomputed rather than cached, so
    /// the estimate cannot drift from the definitions it describes.
    pub fn estimated_tokens(&self) -> usize {
        self.entries.iter().map(|entry| spec_tokens(&entry.spec)).sum()
    }

    fn find(&self, name: &str) -> Option<&dyn ErasedTool> {
        self.entries.iter().find(|entry| entry.spec.name == name).map(|entry| entry.tool.as_ref())
    }

    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    /// Adds a Tool. Fails if a Tool of that name exists.
    ///
    /// # Errors
    ///
    /// Fails if a Tool of that name is already registered.
    pub fn register<T>(&mut self, tool: T) -> Result<(), DuplicateTool>
    where
        T: Tool,
    {
        let spec = tool.spec();
        if self.find(&spec.name).is_some() {
            return Err(DuplicateTool(spec.name));
        }
        self.entries.push(Entry { spec, tool: Box::new(tool) });
        Ok(())
    }

    /// Keeps only the tools whose names `keep` accepts.
    #[inline]
    pub fn retain<Callback>(&mut self, keep: Callback)
    where
        Callback: Fn(&str) -> bool,
    {
        self.entries.retain(|entry| keep(&entry.spec.name));
    }

    #[inline]
    /// Runs the Tool called `name`. An unknown name is an error result.
    pub async fn run(&self, name: &str, raw_arguments: &str, cx: &ToolContext) -> ToolResult {
        match self.find(name) {
            Some(tool) => tool.call(raw_arguments, cx).await,
            None => ToolResult::error(format!("unknown tool `{name}`")),
        }
    }

    /// Whether every call to the Tool called `name` only reads. An unknown tool is not read-only,
    /// which is the cautious answer.
    #[must_use]
    #[inline]
    pub fn is_read_only(&self, name: &str) -> bool {
        self.find(name).is_some_and(ErasedTool::is_read_only)
    }

    /// Whether `value` parses as the arguments of the Tool called `name`, so a test can prove the
    /// schema a Tool shows the model is the arguments it accepts.
    #[cfg(test)]
    pub(crate) fn accepts(&self, name: &str, value: &serde_json::Value) -> bool {
        self.find(name).is_some_and(|tool| tool.accepts(value))
    }

    /// The traits of one call; an unknown tool has the cautious defaults.
    #[must_use]
    #[inline]
    pub fn traits(&self, name: &str, raw_arguments: &str) -> ToolTraits {
        self.find(name).map(|tool| tool.traits_for_call(raw_arguments)).unwrap_or_default()
    }
}

/// The rough tokens a tool definition costs in every request.
fn spec_tokens(spec: &ToolSpec) -> usize {
    let bytes =
        spec.name.len().saturating_add(spec.description.len()).saturating_add(spec.parameters.to_string().len());
    bytes.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tools::tool::Description;
    use core::future;
    use serde::Deserialize;
    use serde_json::{Value, json};

    #[derive(Debug)]
    struct Echo;

    #[derive(Deserialize)]
    struct EchoArgs {
        text: String,
    }

    impl Tool for Echo {
        type Args = EchoArgs;

        fn describe(&self) -> Description {
            Description { description: "Echo the text back", name: "echo" }
        }

        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] })
        }

        fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
            future::ready(ToolResult::ok(format!("echo: {}", args.text)))
        }

        fn summary(&self, args: &EchoArgs) -> String {
            args.text.clone()
        }

        #[inline]
        fn traits_for(&self, _args: &Self::Args) -> ToolTraits {
            self.traits()
        }
    }

    async fn run_set(name: &str, raw: &str) -> ToolResult {
        set().run(name, raw, &ToolContext::detached()).await
    }

    fn set() -> ToolSet {
        let mut set = ToolSet::new();
        set.register(Echo).unwrap();
        set
    }

    #[tokio::test]
    async fn runs_a_registered_tool_with_typed_arguments() {
        let result = run_set("echo", r#"{"text":"hi"}"#).await;
        assert_eq!(result, ToolResult::ok("echo: hi"));
    }

    #[tokio::test]
    async fn unknown_tools_are_error_results() {
        let result = run_set("launch_missiles", "{}").await;
        assert!(result.is_error);
        assert!(result.content.contains("unknown tool `launch_missiles`"));
    }

    #[tokio::test]
    async fn bad_arguments_are_error_results() {
        for raw in ["{not json", r#"{"text":5}"#, "{}", ""] {
            let result = run_set("echo", raw).await;
            assert!(result.is_error, "{raw}");
            assert!(result.content.contains("invalid arguments for `echo`"), "{}", result.content);
        }
    }

    #[test]
    fn asking_whether_a_tool_reads_uses_its_declared_answer_not_its_arguments() {
        // A Tool whose individual calls differ still has to answer without arguments.
        #[derive(Debug)]
        struct Sometimes;

        impl Tool for Sometimes {
            type Args = EchoArgs;

            fn describe(&self) -> Description {
                Description { description: "sometimes reads", name: "sometimes" }
            }

            fn parameters(&self) -> Value {
                json!({ "type": "object", "properties": { "text": { "type": "string" } } })
            }

            fn run(&self, args: EchoArgs, _cx: &ToolContext) -> impl Future<Output = ToolResult> {
                future::ready(ToolResult::ok(args.text))
            }

            fn summary(&self, args: &EchoArgs) -> String {
                args.text.clone()
            }

            fn traits_for(&self, _args: &EchoArgs) -> ToolTraits {
                ToolTraits::read_only()
            }
        }

        let mut set = ToolSet::new();
        set.register(Sometimes).unwrap();
        assert!(!set.is_read_only("sometimes"), "the declared traits decide, not the arguments");
        assert!(!set.is_read_only("unknown"));
    }

    #[test]
    fn the_definitions_have_a_token_estimate_that_grows_with_each_tool() {
        let mut set = ToolSet::new();
        assert_eq!(set.estimated_tokens(), 0);
        set.register(Echo).unwrap();
        assert!(set.estimated_tokens() > 0);
    }

    #[test]
    fn retain_keeps_the_named_tools_and_their_token_estimate() {
        let mut set = set();
        let before = set.estimated_tokens();
        set.retain(|name| name != "echo");
        assert_eq!(set.definitions().count(), 0);
        assert_eq!(set.estimated_tokens(), 0);
        assert!(before > 0);
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let mut set = set();
        assert_eq!(set.register(Echo), Err(DuplicateTool("echo".into())));
    }

    #[test]
    fn definitions_describe_every_tool() {
        let set = set();
        let defs: Vec<&ToolSpec> = set.definitions().collect();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].description, "Echo the text back");
        assert_eq!(defs[0].parameters["required"][0], "text");
    }

    #[test]
    fn describes_calls_for_the_user() {
        let set = set();
        assert_eq!(set.describe("echo", r#"{"text":"hi"}"#), "echo(hi)");
        assert_eq!(set.describe("echo", "{}"), "echo(invalid arguments)");
        assert_eq!(set.describe("nope", "{}"), "nope (unknown tool)");
    }
}
