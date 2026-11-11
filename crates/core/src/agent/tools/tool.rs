use core::fmt::Debug;
use core::future::Future;
use core::pin::Pin;
use tokio::task;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::agent::Settings;
use crate::agent::skills::SkillSet;
use crate::llm::client::ToolSpec;
use crate::llm::client::chat::Usage;

#[derive(Debug, Clone, PartialEq, Eq)]
/// What a Tool returns.
pub struct Outcome {
    /// What the model reads.
    pub content: String,
    /// Whether the call failed.
    pub is_error: bool,
    /// Tokens the call spent on a model of its own, such as a sub-agent's. They count toward the
    /// Turn that ran the call.
    pub usage: Option<Usage>,
}

impl Outcome {
    #[inline]
    pub fn error<IntoString>(message: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self { content: format!("error: {}", message.into()), is_error: true, usage: None }
    }

    #[inline]
    pub fn ok<IntoString>(content: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self { content: content.into(), is_error: false, usage: None }
    }

    #[must_use]
    #[inline]
    pub const fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

impl From<Result<String, String>> for Outcome {
    #[inline]
    fn from(result: Result<String, String>) -> Self {
        match result {
            Ok(content) => Self::ok(content),
            Err(message) => Self::error(message),
        }
    }
}

/// What the agent loop may assume about a tool call, beyond its schema. New flags go here, and
/// the default is the cautious one, so a tool that says nothing is treated as having side effects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Traits {
    /// The call changes nothing, so calls like it can safely run at the same time.
    pub read_only: bool,
}

impl Traits {
    #[must_use]
    #[inline]
    pub const fn read_only() -> Self {
        Self { read_only: true }
    }
}

/// What a running Tool call can use: its own id, the Turn's cancellation, the Agent's current
/// Settings, and a way to show progress on the call's card.
#[derive(Debug)]
#[expect(clippy::partial_pub_fields, reason = "the private fields keep the invariants of the type")]
pub struct Context {
    call_id: String,
    pub cancel: CancellationToken,
    progress: UnboundedSender<ProgressReport>,
    pub settings: Settings,
    pub skill_set: SkillSet,
}

/// A status line from a running Tool call, tagged with the call it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressReport {
    pub call_id: String,
    pub status: String,
}

impl Context {
    #[must_use]
    #[inline]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// A context that nobody listens to, for running a Tool outside an Agent.
    #[must_use]
    #[inline]
    pub fn detached() -> Self {
        let (progress, _) = unbounded_channel();
        Self::new("detached", CancellationToken::new(), Settings::default(), progress, SkillSet::default())
    }

    #[inline]
    /// Creates the context of one Tool call.
    pub fn new<IntoString>(
        call_id: IntoString,
        cancel: CancellationToken,
        settings: Settings,
        progress: UnboundedSender<ProgressReport>,
        skill_set: SkillSet,
    ) -> Self
    where
        IntoString: Into<String>,
    {
        Self { call_id: call_id.into(), cancel, settings, progress, skill_set }
    }

    /// Shows `status` on this call's card. A report that arrives after the Turn has ended is
    /// dropped.
    #[inline]
    pub fn report<IntoString>(&self, status: IntoString)
    where
        IntoString: Into<String>,
    {
        let _unused = self.progress.send(ProgressReport { call_id: self.call_id.clone(), status: status.into() });
    }
}

/// Something an Agent can call.
pub trait Tool: Debug + Send + Sync + 'static {
    /// The arguments the Tool takes.
    type Args: DeserializeOwned + Send + 'static;

    fn description(&self) -> &'static str;

    fn name(&self) -> &'static str;

    /// JSON schema of the arguments, as shown to the model.
    fn parameters(&self) -> Value;

    fn run(&self, args: Self::Args, cx: &Context) -> impl Future<Output = Outcome> + Send;

    /// A short description of one call, for showing it to the user.
    fn summary(&self, args: &Self::Args) -> String;

    /// The traits of every call to this tool. A tool whose calls differ makes
    /// [`traits_for`](Self::traits_for) look at the arguments instead.
    fn traits(&self) -> Traits;

    /// The traits of one call, given its parsed arguments.
    fn traits_for(&self, args: &Self::Args) -> Traits;
}

/// A Tool with its arguments type erased.
pub(super) trait ErasedTool: Debug + Send + Sync {
    fn call<'call>(
        &'call self,
        raw_arguments: &'call str,
        cx: &'call Context,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'call>>;
    fn describe_call(&self, raw_arguments: &str) -> String;
    fn spec(&self) -> ToolSpec;
    fn tool_name(&self) -> &'static str;
    fn traits_for_call(&self, raw_arguments: &str) -> Traits;
}

impl<T: Tool> ErasedTool for T {
    fn call<'call>(
        &'call self,
        raw_arguments: &'call str,
        cx: &'call Context,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'call>> {
        Box::pin(async move {
            let schema = self.parameters();
            match parse_coercing::<T::Args>(raw_arguments, &schema) {
                Ok(args) => self.run(args, cx).await,
                // The schema goes with the error, so the model can correct the call in one step.
                Err(e) => Outcome::error(format!(
                    "invalid arguments for `{}`: {e}. The arguments must match this schema: {schema}",
                    Tool::name(self)
                )),
            }
        })
    }

    fn describe_call(&self, raw_arguments: &str) -> String {
        parse_coercing::<T::Args>(raw_arguments, &self.parameters())
            .map_or_else(|_| "invalid arguments".to_owned(), |args| self.summary(&args))
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: Tool::name(self).to_owned(),
            description: self.description().to_owned(),
            parameters: self.parameters(),
        }
    }

    fn tool_name(&self) -> &'static str {
        Tool::name(self)
    }

    /// Arguments that do not parse never run, so their call gets the static traits.
    fn traits_for_call(&self, raw_arguments: &str) -> Traits {
        parse_coercing::<T::Args>(raw_arguments, &self.parameters())
            .map_or_else(|_| Tool::traits(self), |args| self.traits_for(&args))
    }
}

fn parse_arguments<A>(raw: &str) -> Result<A, String>
where
    A: DeserializeOwned,
{
    let trimmed = raw.trim();
    let parsed: Value = if trimmed.is_empty() {
        json!({})
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("not valid JSON: {e}"))?
    };
    let value = if parsed.is_null() { json!({}) } else { parsed };
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Parses the arguments; if that fails, fixes the kinds of mistakes models make with types and
/// tries again: a number or boolean sent as a string, or an array or object sent as JSON text.
/// The error is the first one, which describes the call as it was sent.
fn parse_coercing<A>(raw: &str, schema: &Value) -> Result<A, String>
where
    A: DeserializeOwned,
{
    let first = match parse_arguments::<A>(raw) {
        Ok(args) => return Ok(args),
        Err(e) => e,
    };
    let Ok(mut value) = serde_json::from_str::<Value>(raw.trim()) else {
        return Err(first);
    };
    coerce(&mut value, schema);
    serde_json::from_value(value).map_err(|_retry_error| first)
}

/// Fixes values a model sent as the wrong type, in place, following `schema`.
fn coerce(value: &mut Value, schema: &Value) {
    let kind = schema.get("type").and_then(Value::as_str);
    let replacement = match (kind, value.as_str()) {
        (Some("integer"), Some(text)) => text.trim().parse::<i64>().ok().map(Value::from),
        (Some("number"), Some(text)) => {
            text.trim().parse::<i64>().map(Value::from).or_else(|_| text.trim().parse::<f64>().map(Value::from)).ok()
        },
        (Some("boolean"), Some(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        (Some("array"), Some(text)) => serde_json::from_str::<Value>(text).ok().filter(Value::is_array),
        (Some("object"), Some(text)) => serde_json::from_str::<Value>(text).ok().filter(Value::is_object),
        _ => None,
    };
    if let Some(fixed) = replacement {
        *value = fixed;
    }
    match *value {
        Value::Object(ref mut map) => {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (key, inner) in map.iter_mut() {
                    if let Some(inner_schema) = properties.get(key) {
                        coerce(inner, inner_schema);
                    }
                }
            }
        },
        Value::Array(ref mut items) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items {
                    coerce(item, item_schema);
                }
            }
        },
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {},
    }
}

pub(super) async fn blocking<F>(work: F) -> Outcome
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    match task::spawn_blocking(work).await {
        Ok(result) => result.into(),
        Err(e) => Outcome::error(format!("tool crashed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Args {
        #[serde(default)]
        count: Option<u32>,
        name: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct AllDefaults {
        #[serde(default)]
        path: Option<String>,
    }

    #[test]
    fn parses_json_text() {
        let args: Args = parse_arguments(r#"{"name":"x","count":2}"#).unwrap();
        assert_eq!(args, Args { name: "x".into(), count: Some(2) });
    }

    #[test]
    fn blank_and_null_mean_no_arguments() {
        assert_eq!(parse_arguments::<AllDefaults>("").unwrap(), AllDefaults { path: None });
        assert_eq!(parse_arguments::<AllDefaults>("  ").unwrap(), AllDefaults { path: None });
        assert_eq!(parse_arguments::<AllDefaults>("null").unwrap(), AllDefaults { path: None });
    }

    #[test]
    fn reports_malformed_json_and_wrong_types() {
        assert!(parse_arguments::<Args>("{not json").unwrap_err().contains("not valid JSON"));
        parse_arguments::<Args>(r#"{"name":5}"#).unwrap_err();
        parse_arguments::<Args>("{}").unwrap_err();
    }

    #[test]
    fn error_results_are_marked_and_prefixed() {
        let result = Outcome::error("boom");
        assert!(result.is_error);
        assert_eq!(result.content, "error: boom");
        assert!(!Outcome::ok("fine").is_error);
    }

    #[test]
    fn numbers_and_json_sent_as_text_are_fixed_using_the_schema() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Wide {
            line: u32,
            list: Vec<String>,
            on: bool,
        }
        let schema = json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer" },
                "list": { "type": "array", "items": { "type": "string" } },
                "on": { "type": "boolean" }
            }
        });
        let raw = r#"{"line":"12","list":"[\"a\"]","on":"True"}"#;
        parse_arguments::<Wide>(raw).unwrap_err();
        assert_eq!(parse_coercing::<Wide>(raw, &schema).unwrap(), Wide { line: 12, list: vec!["a".into()], on: true });
        let err = parse_coercing::<Wide>(r#"{"line":"x"}"#, &schema).unwrap_err();
        assert!(err.contains("invalid type"), "the first error is kept: {err}");
    }
}
