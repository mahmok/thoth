use alloc::fmt;
use core::error;

use crate::llm::client::ToolSpec;

use super::tool::{Context as ToolContext, ErasedTool, Outcome as ToolResult, Tool, Traits as ToolTraits};

#[derive(Debug, PartialEq, Eq)]
/// Returned when a Tool name is registered twice.
pub struct DuplicateTool(pub String);

impl fmt::Display for DuplicateTool {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a tool named `{}` is already registered", self.0)
    }
}

impl error::Error for DuplicateTool {
    #[inline]
    fn cause(&self) -> Option<&dyn error::Error> {
        None
    }

    #[inline]
    fn description(&self) -> &'static str {
        "a Tool name was registered twice"
    }

    #[inline]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

#[derive(Default, Debug)]
/// The Tools an Agent can call.
pub struct ToolSet {
    specs: Vec<ToolSpec>,
    tokens: usize,
    //TODO: Maybe convert to hashmap or tree.
    tools: Vec<Box<dyn ErasedTool>>,
}

impl ToolSet {
    #[must_use]
    #[inline]
    pub fn definitions(&self) -> &[ToolSpec] {
        &self.specs
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
    pub const fn estimated_tokens(&self) -> usize {
        self.tokens
    }

    fn find(&self, name: &str) -> Option<&dyn ErasedTool> {
        self.tools.iter().map(AsRef::as_ref).find(|t| t.tool_name() == name)
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
        let name = Tool::name(&tool);
        if self.find(name).is_some() {
            return Err(DuplicateTool(name.to_owned()));
        }
        let spec = tool.spec();
        self.tokens = self.tokens.saturating_add(spec_tokens(&spec));
        self.specs.push(spec);
        self.tools.push(Box::new(tool));
        Ok(())
    }

    /// Keeps only the tools whose names `keep` accepts.
    #[inline]
    pub fn retain<Callback>(&mut self, keep: Callback)
    where
        Callback: Fn(&str) -> bool,
    {
        self.tools.retain(|tool| keep(tool.tool_name()));
        self.specs.retain(|spec| keep(&spec.name));
        self.tokens = self.specs.iter().map(spec_tokens).sum();
    }

    #[inline]
    /// Runs the Tool called `name`. An unknown name is an error result.
    pub async fn run(&self, name: &str, raw_arguments: &str, cx: &ToolContext) -> ToolResult {
        match self.find(name) {
            Some(tool) => tool.call(raw_arguments, cx).await,
            None => ToolResult::error(format!("unknown tool `{name}`")),
        }
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

        fn description(&self) -> &'static str {
            "Echo the text back"
        }

        fn name(&self) -> &'static str {
            "echo"
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
        fn traits(&self) -> ToolTraits {
            ToolTraits::default()
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
        assert_eq!(set.definitions(), []);
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
        let defs = set.definitions();
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
