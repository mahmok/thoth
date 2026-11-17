//! One Turn: the model steps, the Tool calls they ask for, and the events both report.
//!
//! The Turn asks [`ContextBudget`](crate::domain::conversation::budget::ContextBudget) what to do about its
//! context; it does not work the thresholds out itself.

use core::fmt::Write as _;
use core::future::Future;
use std::collections::HashSet;

use futures_util::future::join_all;
use tokio::select;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

use super::{Agent, AgentEvent, Failure, Settings, until};
use crate::adapters::llm::client::{CreateCompletionRequest, Delta, StreamedCompletion, ToolChoice, ToolSpec};
use crate::domain::conversation::budget::Elision;
use crate::domain::conversation::budget::{Decision, Load, Request};
use crate::domain::conversation::{Conversation, Turn};
use crate::domain::message::{AgentMessage, ToolCall};
use crate::domain::shared::Usage;
use crate::domain::tool::Outcome as ToolResult;
use crate::runtime::tools::tool::{ProgressReport, ToolContext};
use crate::runtime::types::AgentResponse;

/// How many steps before the limit the model starts being told that few are left.
const STEPS_LEFT_WARNING: usize = 3;

/// Added to a failed tool call that is identical to one that already failed in the Turn.
const REPEATED_FAILURE: &str = "\n[this exact call already failed earlier in this turn; if nothing has changed since, repeating it will fail again. Change the call, or read the file again first]";

impl Agent {
    #[inline]
    /// Runs a Turn for `prompt`, reporting events as they happen.
    ///
    /// # Errors
    ///
    /// Fails if no model is chosen, the model request fails, the turn is interrupted or it runs out of steps.
    pub async fn respond<F>(&self, prompt: String, on_event: F) -> Result<AgentResponse, Failure>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.respond_under(&CancellationToken::new(), prompt, on_event).await
    }

    /// Like [`respond`](Self::respond), but the Turn also ends when `parent` is cancelled. A
    /// Sub-agent's Turn runs this way, so cancelling the Root agent's Turn cancels it too.
    #[inline]
    /// Runs a Turn that a parent's cancellation also stops.
    ///
    /// # Errors
    ///
    /// Fails as `respond` does.
    #[expect(clippy::significant_drop_tightening, reason = "the guard is deliberately held for the whole operation")]
    pub async fn respond_under<F>(
        &self,
        parent: &CancellationToken,
        prompt: String,
        mut on_event: F,
    ) -> Result<AgentResponse, Failure>
    where
        F: FnMut(AgentEvent) + Send,
    {
        drop(self.client()?);
        // Registered before waiting for the Conversation, so an interrupt during the wait counts.
        let (cancelled, _active) = self.new_cancellation(parent);
        let mut conversation = self.conversation.lock().await;
        let usage = until(&cancelled, self.shrink(&mut conversation, &prompt, &mut on_event)).await?;
        let mut turn = conversation.begin_turn(prompt);
        let result = self.run_loop(&mut turn, usage, &cancelled, on_event).await;
        match result {
            Err(Failure::Interrupted) => turn.keep_partial(),
            // The files stay changed when the Turn fails, so the model must still know.
            Err(_) => turn.keep_if_changed(),
            Ok(_) => {},
        }
        result
    }

    /// Runs one Tool call with its context. A call whose reply was cut off mid-argument is not
    /// run: the model is told, and can try again with less.
    async fn run_call(&self, call: &ToolCall, cx: ToolContext, cut_off: bool) -> ToolResult {
        let (name, arguments) = (&call.function.name, &call.function.arguments);
        if cut_off && serde_json::from_str::<serde_json::Value>(arguments).is_err() {
            return ToolResult::error(
                "your reply was cut off while writing this tool call, so it was not run; try again with a smaller call",
            );
        }
        self.tools.run(name, arguments, &cx).await
    }

    /// Like [`run_call`](Self::run_call), but a cancel ends it with an error result instead of
    /// dropping the whole step. The step is then still recorded, so that what its other calls
    /// did is not lost.
    async fn run_call_guarded(
        &self,
        call: &ToolCall,
        cx: ToolContext,
        cut_off: bool,
        cancel: &CancellationToken,
    ) -> ToolResult {
        select! {
            biased;
            () = cancel.cancelled() => ToolResult::error(
                "interrupted by the user before this call finished; it may or may not have taken effect",
            ),
            result = self.run_call(call, cx, cut_off) => result,
        }
    }

    /// Runs the model and Tool steps of a Turn until the model answers, up to the step limit.
    async fn run_loop<F>(
        &self,
        turn: &mut Turn<'_>,
        mut usage: Option<Usage>,
        cancel: &CancellationToken,
        mut on_event: F,
    ) -> Result<AgentResponse, Failure>
    where
        F: FnMut(AgentEvent) + Send,
    {
        // The calls that failed in this Turn, to notice the model repeating one.
        let mut failed: HashSet<(String, String)> = HashSet::new();
        for step in 1..=self.max_steps {
            // Checked here too, not only where the work awaits: a cancel that came during the
            // last Tool calls must not start another model request.
            if cancel.is_cancelled() {
                return Err(Failure::Interrupted);
            }
            on_event(AgentEvent::StepStarted { step });
            self.trim(turn);
            let sent = self.load(turn.estimated_tokens(), 0).raw();
            // The last step must answer in text, so the Turn ends with a reply that says what was
            // done and what is left, not with an error. A Turn of one step has no such choice.
            let tool_choice =
                if step == self.max_steps && self.max_steps > 1 { ToolChoice::None } else { ToolChoice::Auto };
            let completion = until(cancel, self.step(turn, tool_choice, &mut on_event)).await?;

            self.learn(sent, completion.usage.as_ref());
            usage = add_usage(usage, completion.usage);

            if completion.tool_calls.is_empty() {
                if let Some(reason) = completion.cut_off.as_ref() {
                    on_event(AgentEvent::Notice(format!(
                        "the reply was cut off ({reason}); say \"continue\" to go on"
                    )));
                }
                let message = AgentMessage::new(Some(completion.text), completion.reasoning);
                turn.complete(message.clone());
                return Ok(AgentResponse { message, usage });
            }

            let msg = AgentMessage::new((!completion.text.is_empty()).then_some(completion.text), completion.reasoning);
            let settings = self.settings();
            let changes = !completion
                .tool_calls
                .iter()
                .all(|call| self.tools.traits(&call.function.name, &call.function.arguments).read_only);
            let results = self
                .run_tools(&completion.tool_calls, completion.cut_off.is_some(), cancel, &settings, &mut on_event)
                .await;
            usage = results.iter().fold(usage, |total, result| add_usage(total, result.usage));
            let mut executed: Vec<(ToolCall, ToolResult)> = completion
                .tool_calls
                .into_iter()
                .zip(results)
                .map(|(call, mut result)| {
                    let key = (call.function.name.clone(), call.function.arguments.clone());
                    if result.is_error && !failed.insert(key) {
                        result.content.push_str(REPEATED_FAILURE);
                    }
                    (call, result)
                })
                .collect();
            let remaining = self.max_steps.saturating_sub(step);
            if (1..=STEPS_LEFT_WARNING).contains(&remaining)
                && let Some(last) = executed.last_mut()
            {
                let _written = write!(
                    last.1.content,
                    "\n[{remaining} model step{} left in this turn: stop exploring, finish the work and give the user a final answer]",
                    if remaining == 1 { "" } else { "s" }
                );
            }

            turn.record_tool_step(msg, executed);
            if changes {
                turn.note_changes();
            }
        }

        turn.keep_partial();
        Err(Failure::StepLimit(self.max_steps))
    }

    /// Runs the calls of one step and reports each as it starts, progresses and finishes. A step
    /// of read-only calls runs them at the same time; anything else runs them one by one, in order.
    async fn run_tools<F>(
        &self,
        calls: &[ToolCall],
        cut_off: bool,
        cancel: &CancellationToken,
        settings: &Settings,
        on_event: &mut F,
    ) -> Vec<ToolResult>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let (progress, mut reports) = unbounded_channel();
        let context = |call: &ToolCall| {
            ToolContext::new(&call.id, cancel.clone(), settings.clone(), progress.clone(), self.skills.clone())
        };
        let summaries: Vec<String> =
            calls.iter().map(|call| self.tools.describe(&call.function.name, &call.function.arguments)).collect();
        let in_parallel = calls.len() > 1
            && calls.iter().all(|call| self.tools.traits(&call.function.name, &call.function.arguments).read_only);

        let mut results = Vec::with_capacity(calls.len());
        if in_parallel {
            for (call, summary) in calls.iter().zip(&summaries) {
                on_event(started(call, summary));
            }
            let work = join_all(calls.iter().map(|call| self.run_call_guarded(call, context(call), cut_off, cancel)));
            let finished = pump(work, &mut reports, on_event).await;
            for ((call, summary), result) in calls.iter().zip(&summaries).zip(finished) {
                on_event(finish(call, summary, &result));
                results.push(result);
            }
        } else {
            for (call, summary) in calls.iter().zip(&summaries) {
                on_event(started(call, summary));
                let work = self.run_call_guarded(call, context(call), cut_off, cancel);
                let result = pump(work, &mut reports, on_event).await;
                on_event(finish(call, summary, &result));
                results.push(result);
            }
        }
        results
    }

    /// Elides or compacts before a request when the next one would not fit the context.
    async fn shrink<F>(
        &self,
        conversation: &mut Conversation,
        prompt: &str,
        on_event: &mut F,
    ) -> Result<Option<Usage>, Failure>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let limit = self.model().and_then(|model| model.context_limit());
        // The prompt counts: a large one, with files attached, is part of the next request.
        let incoming = prompt.len().div_ceil(4);
        let mut elided = false;
        loop {
            let load = self.load(conversation.estimated_tokens(), incoming);
            let request = Request::before_turn(load, self.calibration(), limit);
            let request = if elided { request.elided() } else { request };
            match self.budget.decide(&request) {
                Decision::Keep => return Ok(None),
                Decision::Shrink(rules) => {
                    conversation.elide(&rules);
                    elided = true;
                },
                Decision::Compact(rules) => {
                    return Ok(self
                        .compact_conversation(conversation, rules.keep_recent, None, on_event)
                        .await
                        .unwrap_or(None));
                },
            }
        }
    }

    /// The tokens a request over `conversation` tokens would carry, with `incoming` not yet in it.
    fn load(&self, conversation: usize, incoming: usize) -> Load {
        Load::new(conversation, self.tools.estimated_tokens()).with_incoming(incoming)
    }

    /// Asks the model for one reply, recovering once from a context overflow by eliding.
    async fn step<F>(
        &self,
        turn: &mut Turn<'_>,
        tool_choice: ToolChoice,
        on_event: &mut F,
    ) -> Result<StreamedCompletion, Failure>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut recovered = false;
        loop {
            let client = self.client()?;
            let definitions: Vec<ToolSpec> = self.tools.definitions().cloned().collect();
            let result = client
                .create_completion_stream(
                    CreateCompletionRequest {
                        messages: turn.messages(),
                        tools: &definitions,
                        thinking: self.thinking(),
                        tool_choice,
                    },
                    &mut |delta| match delta {
                        Delta::Text(text) => on_event(AgentEvent::Text(text.to_owned())),
                        Delta::Reasoning(text) => on_event(AgentEvent::Reasoning(text.to_owned())),
                    },
                )
                .await;
            match result {
                Err(e) if !recovered && e.is_context_overflow() && turn.elide(&Elision::EVERYTHING) => {
                    recovered = true;
                    on_event(AgentEvent::Notice(
                        "the conversation was too long: dropped old tool output and retrying".into(),
                    ));
                },
                other => return other.map_err(Failure::Client),
            }
        }
    }

    fn trim(&self, turn: &mut Turn<'_>) {
        let limit = self.model().and_then(|model| model.context_limit());
        let load = self.load(turn.estimated_tokens(), 0);
        let request = Request::between_steps(load, self.calibration(), limit);
        if let Decision::Shrink(rules) = self.budget.decide(&request) {
            let _elided = turn.elide(&rules);
        }
    }
}

/// Adds one step's usage to a Turn's total. Either side may be absent.
fn add_usage(total: Option<Usage>, step: Option<Usage>) -> Option<Usage> {
    match (total, step) {
        (Some(so_far), Some(latest)) => Some(so_far.merge(latest)),
        (so_far, latest) => so_far.or(latest),
    }
}

fn started(call: &ToolCall, summary: &str) -> AgentEvent {
    AgentEvent::ToolStarted { id: call.id.clone(), summary: summary.to_owned() }
}

fn finish(call: &ToolCall, summary: &str, result: &ToolResult) -> AgentEvent {
    AgentEvent::ToolFinished { id: call.id.clone(), summary: summary.to_owned(), result: result.clone() }
}

/// Runs `work` to the end, passing on the progress its Tool calls report while it runs.
async fn pump<T, F>(
    work: impl Future<Output = T>,
    reports: &mut UnboundedReceiver<ProgressReport>,
    on_event: &mut F,
) -> T
where
    F: FnMut(AgentEvent),
{
    tokio::pin!(work);
    let output = loop {
        select! {
            biased;
            output = &mut work => break output,
            Some(report) = reports.recv() => on_event(progress(report)),
        }
    };
    while let Ok(report) = reports.try_recv() {
        on_event(progress(report));
    }
    output
}

fn progress(report: ProgressReport) -> AgentEvent {
    AgentEvent::ToolProgress { id: report.call_id, status: report.status }
}

#[cfg(test)]
mod tests;
