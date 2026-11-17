use super::tool::{Description, Outcome, Tool, ToolContext, Traits as ToolTraits, blocking};
use crate::agent::skills::SkillSet;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub(super) struct InvokeSkill {}

#[derive(Debug, Deserialize)]
pub(super) struct InvokeSkillArgs {
    name: String,
}

impl Tool for InvokeSkill {
    type Args = InvokeSkillArgs;

    fn describe(&self) -> Description {
        Description { description: "Invokes a skill by name, add it to the session context", name: "invoke_skill" }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "name of the skill to invoke" },
            },
            "required": ["name"]
        })
    }

    async fn run(&self, args: InvokeSkillArgs, cx: &ToolContext) -> Outcome {
        let skill_set = cx.skill_set.clone();
        blocking(move || get_skill_block(&skill_set, &args.name)).await
    }

    fn summary(&self, args: &InvokeSkillArgs) -> String {
        format!("invoke skill: {}", args.name)
    }

    fn traits(&self) -> ToolTraits {
        ToolTraits::read_only()
    }
}

fn get_skill_block(skill_set: &SkillSet, name: &str) -> Result<String, String> {
    skill_set.get_skill_by_name(name).map_or_else(|| Err(format!("Skill {name} not found")), |skill| Ok(skill.block()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Settings;
    use crate::agent::skills::Skill;
    use crate::agent::tools::testing::run_with_context;
    use std::path::PathBuf;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio_util::sync::CancellationToken;

    fn skill(name: &str, body: &str) -> Skill {
        Skill {
            body: body.to_string(),
            name: name.to_string(),
            description: "Test skill description".to_string(),
            path: PathBuf::from("test_skill.md"),
        }
    }

    fn skill_a() -> Skill {
        skill("Skill A", "Skill A body")
    }

    fn skill_b() -> Skill {
        skill("Skill B", "Skill B body")
    }

    fn tool() -> InvokeSkill {
        InvokeSkill::default()
    }

    fn context() -> ToolContext {
        let mut skill_set = SkillSet::default();
        skill_set.push(skill_a());
        skill_set.push(skill_b());
        let (progress, _) = unbounded_channel();
        ToolContext::new("detached", CancellationToken::new(), Settings::default(), progress, skill_set)
    }

    #[tokio::test]
    async fn invokes_a_skill() {
        let result = run_with_context(tool(), json!({ "name": "Skill B" }), &context()).await;
        assert_eq!(result, Outcome::ok(skill_b().block()));
    }

    #[tokio::test]
    async fn returns_error_when_skill_doesnt_exist() {
        let result = run_with_context(tool(), json!({ "name": "Skill C" }), &context()).await;
        assert_eq!(result, Outcome::error("Skill Skill C not found"));
    }
}
