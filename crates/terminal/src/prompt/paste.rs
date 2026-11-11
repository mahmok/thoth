use ratatui_textarea::TextArea;

#[derive(Default)]
/// Pasted text behind placeholders.
pub(super) struct PasteStore {
    counter: usize,
    entries: Vec<(String, String)>,
}

impl PasteStore {
    pub(super) fn expand(&self, prompt: &str) -> String {
        self.entries.iter().fold(prompt.to_owned(), |acc, entry| acc.replace(entry.0.as_str(), &entry.1))
    }

    pub(super) fn insert(&mut self, textarea: &mut TextArea<'_>, pasted: &str) {
        let text = pasted.replace("\r\n", "\n").replace('\r', "\n");

        let line_count = text.lines().count();
        if line_count <= 1 {
            let _inserted = textarea.insert_str(text.trim_end_matches('\n'));
            return;
        }

        self.counter = self.counter.saturating_add(1);
        let placeholder = format!("[Pasted {line_count} lines #{}]", self.counter);
        let _inserted = textarea.insert_str(&placeholder);
        self.entries.push((placeholder, text));
    }

    /// Forgets the pastes whose placeholder is no longer in `text`.
    pub(super) fn retain_present(&mut self, text: &str) {
        self.entries.retain(|entry| text.contains(entry.0.as_str()));
        if self.entries.is_empty() {
            self.counter = 0;
        }
    }
}
