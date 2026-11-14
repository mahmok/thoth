pub mod fetch;
pub mod refresh;
pub mod store;
#[cfg(any(test, feature = "testing"))]
#[expect(clippy::unwrap_used, reason = "test support: a failed setup should fail the test")]
pub mod testing;
pub mod worker;

use alloc::sync::Arc;

use store::Models as ModelsStore;

use crate::llm::provider::ModelSpec;

#[inline]
/// Every stored model with a display name, for the model picker.
pub fn choices(store: &dyn ModelsStore) -> Vec<(ModelSpec, String)> {
    store
        .providers()
        .into_iter()
        .flat_map(|provider| {
            let models = provider.models.clone();
            models.into_iter().map(move |model| {
                let name = model.display_name().to_owned();
                (ModelSpec::new(Arc::clone(&provider), model), name)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::Protocol;
    use crate::test::assert_empty;
    use testing::{fixture, store_with};

    #[test]
    fn choices_follow_the_provider_order_and_keep_each_models_protocol() {
        let found: Vec<(String, Protocol, String)> = choices(fixture().as_ref())
            .into_iter()
            .map(|(spec, _)| (spec.provider.id.clone(), spec.protocol(), spec.model.id))
            .collect();
        assert_eq!(found.len(), 8);
        assert_eq!(found[0], ("ollama".into(), Protocol::OpenAi, "bonsai-2:2b".to_owned()));
        assert_eq!(found[5], ("anthropic".into(), Protocol::Anthropic, "claude-sonnet-4-5".to_owned()));
        assert_eq!(found[6], ("github-copilot".into(), Protocol::Anthropic, "claude-sonnet-4.5".to_owned()));
    }

    #[test]
    fn an_empty_store_has_no_choices() {
        assert_empty(choices(store_with(&[]).as_ref()));
    }
}
