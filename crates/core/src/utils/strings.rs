/// Checks if the given character is a valid name character (alphanumeric, hyphen, or underscore).
#[must_use]
#[inline]
pub fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// Expands tokens in the given text that start with the specified prefix.
/// The lookup function is used to retrieve the replacement string for each token.
#[inline]
pub fn expand<Callback>(prefix: &str, text: &str, lookup: Callback) -> String
where
    Callback: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut at_token_start = true;

    while let Some(c) = rest.chars().next() {
        if at_token_start && let Some(after) = rest.strip_prefix(prefix) {
            let name_len = after.find(|ch: char| !is_name_char(ch)).unwrap_or(after.len());
            let (name, tail) = after.split_at_checked(name_len).unwrap_or((after, ""));
            if let Some(block) = (!name.is_empty()).then(|| lookup(name)).flatten() {
                out.push_str(&block);
                rest = tail;
                at_token_start = false;
                continue;
            }
        }
        out.push(c);
        at_token_start = c.is_whitespace();
        rest = rest.get(c.len_utf8()..).unwrap_or_default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL_PREFIX: &str = "/skill:";

    fn lookup(name: &str) -> Option<String> {
        matches!(name, "tdd" | "pr").then(|| format!("<{name}>"))
    }

    #[test]
    fn a_reference_is_replaced_by_its_block() {
        assert_eq!(expand(SKILL_PREFIX, "/skill:tdd fix the bug", lookup), "<tdd> fix the bug");
    }

    #[test]
    fn several_references_expand_anywhere_in_the_text() {
        assert_eq!(expand(SKILL_PREFIX, "use /skill:tdd then /skill:pr.", lookup), "use <tdd> then <pr>.");
    }

    #[test]
    fn unknown_names_stay_as_typed() {
        assert_eq!(expand(SKILL_PREFIX, "/skill:nope hi /skill: x", lookup), "/skill:nope hi /skill: x");
    }

    #[test]
    fn a_reference_in_the_middle_of_a_word_is_left_alone() {
        assert_eq!(expand(SKILL_PREFIX, "see src/skill:tdd", lookup), "see src/skill:tdd");
    }

    #[test]
    fn text_without_references_is_unchanged() {
        assert_eq!(expand(SKILL_PREFIX, "h\u{e9}llo w\u{f6}rld\nline", lookup), "h\u{e9}llo w\u{f6}rld\nline");
    }
}
