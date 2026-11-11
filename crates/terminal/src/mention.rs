/// A trigger character at the start of a word, and what has been typed after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// Characters from the trigger up to the cursor.
    pub len: usize,
    pub query: String,
    pub trigger: char,
}

/// `cursor` counts characters from the start of `text`.
pub fn mention_at(text: &str, cursor: usize, triggers: &[char]) -> Option<Mention> {
    let before: Vec<char> = text.chars().take(cursor).collect();
    let at = before.iter().enumerate().rev().find(|&(index, c)| {
        triggers.contains(c)
            && (index == 0 || before.get(index.saturating_sub(1)).is_some_and(|previous| previous.is_whitespace()))
    })?;
    let (start, &trigger) = at;
    let rest = before.get(start.saturating_add(1)..)?;
    let query: String = match rest.split_first() {
        Some((&'"', quoted)) if !quoted.contains(&'"') => quoted.iter().collect(),
        Some((&'"', _)) => return None,
        _ if rest.iter().any(|c| c.is_whitespace()) => return None,
        _ => rest.iter().collect(),
    };
    Some(Mention { trigger, query, len: before.len().saturating_sub(start) })
}

/// The text and cursor after the mention ending at `cursor` is replaced by `replacement`.
pub fn replace_mention(text: &str, cursor: usize, triggers: &[char], replacement: &str) -> Option<(String, usize)> {
    let mention = mention_at(text, cursor, triggers)?;
    let chars: Vec<char> = text.chars().collect();
    let end = cursor.min(chars.len());
    let start = end.saturating_sub(mention.len);
    let mut replaced: String = chars.get(..start)?.iter().collect();
    replaced.push_str(replacement);
    replaced.extend(chars.get(end..)?);
    Some((replaced, start.saturating_add(replacement.chars().count())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: &[char] = &['@'];

    fn query(text: &str) -> Option<String> {
        mention_at(text, text.chars().count(), AT).map(|m| m.query)
    }

    #[test]
    fn a_trigger_at_the_start_of_a_token_opens_a_mention() {
        assert_eq!(query("@"), Some(String::new()));
        assert_eq!(query("look at @src/ma"), Some("src/ma".into()));
        assert_eq!(mention_at("a @bc", 5, AT).unwrap().len, 3);
    }

    #[test]
    fn a_trigger_inside_a_word_is_not_a_mention() {
        assert_eq!(query("mail me@host"), None);
        assert_eq!(query("plain text"), None);
    }

    #[test]
    fn a_finished_token_is_not_a_mention() {
        assert_eq!(query("@a.rs "), None);
        assert_eq!(query("@a.rs and more"), None);
    }

    #[test]
    fn a_quoted_mention_may_contain_spaces_until_it_closes() {
        assert_eq!(query("@\"my no"), Some("my no".into()));
        assert_eq!(query("@\"my notes.txt\""), None);
    }

    #[test]
    fn any_of_the_triggers_opens_a_mention_and_is_reported() {
        let mention = mention_at("see #12", 7, &['@', '#']).unwrap();
        assert_eq!((mention.trigger, mention.query.as_str()), ('#', "12"));
        assert_eq!(mention_at("see #12", 7, AT), None);
    }

    #[test]
    fn a_mention_is_found_before_the_cursor_wherever_the_cursor_is() {
        assert_eq!(mention_at("look @sr and more", 8, AT).unwrap().query, "sr");
        assert!(mention_at("look @sr and more", 17, AT).is_none());
    }

    #[test]
    fn replacing_a_mention_keeps_the_text_around_it_and_moves_the_cursor_after_it() {
        assert_eq!(
            replace_mention("see @sr and more", 7, AT, "@src/main.rs "),
            Some(("see @src/main.rs  and more".to_owned(), 17))
        );
        assert_eq!(replace_mention("no mention", 4, AT, "@x"), None);
    }
}
