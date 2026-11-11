use crate::llm::client::chat::{Message, Role};

use super::credential::now_ms;

/// The user agent Copilot expects.
pub(super) const USER_AGENT: &str = "GitHubCopilotChat/0.35.0";
/// Headers sent with every Copilot request.
pub(super) const FIXED_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", USER_AGENT),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
/// How long before expiry a token is renewed.
pub(super) const REFRESH_MARGIN_MS: u64 = 5 * 60 * 1000;

#[must_use]
#[inline]
pub fn expires_soon(expires: u64) -> bool {
    now_ms().saturating_add(REFRESH_MARGIN_MS) >= expires
}

#[must_use]
#[inline]
/// The API host a Copilot token names, if it names one.
pub fn base_url_from_token(token: &str) -> Option<String> {
    let rest = token.split("proxy-ep=").nth(1)?;
    let host = rest.split(';').next().filter(|host| !host.is_empty())?;
    let api_host = host.strip_prefix("proxy.").map_or_else(|| host.to_owned(), |tail| format!("api.{tail}"));
    Some(format!("https://{api_host}"))
}

#[must_use]
#[inline]
/// The headers of a Copilot request, marking who started the turn.
pub fn headers(messages: &[Message]) -> Vec<(&'static str, String)> {
    let mut headers: Vec<(&'static str, String)> =
        FIXED_HEADERS.iter().map(|&(name, value)| (name, value.to_owned())).collect();
    let initiator = match messages.last() {
        Some(last) if last.role != Role::User => "agent",
        _ => "user",
    };
    headers.push(("X-Initiator", initiator.to_owned()));
    headers.push(("Openai-Intent", "conversation-edits".to_owned()));
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_host_comes_from_the_token() {
        let token = "tid=1;exp=2;proxy-ep=proxy.individual.githubcopilot.com;sku=x";
        assert_eq!(base_url_from_token(token).as_deref(), Some("https://api.individual.githubcopilot.com"));
        assert_eq!(base_url_from_token("tid=1;exp=2"), None);
        assert_eq!(base_url_from_token("proxy-ep=;x"), None);
    }

    #[test]
    fn requests_are_user_or_agent_initiated() {
        let initiator =
            |messages: &[Message]| headers(messages).into_iter().find(|&(name, _)| name == "X-Initiator").unwrap().1;
        assert_eq!(initiator(&[Message::user("hi")]), "user");
        assert_eq!(initiator(&[Message::user("hi"), Message::tool("c", "out")]), "agent");
        assert_eq!(initiator(&[]), "user");
        let all = headers(&[]);
        assert!(all.contains(&("Copilot-Integration-Id", "vscode-chat".to_owned())));
        assert!(all.contains(&("Openai-Intent", "conversation-edits".to_owned())));
    }

    #[test]
    fn tokens_expire_five_minutes_early() {
        assert!(expires_soon(now_ms() + 60_000));
        assert!(!expires_soon(now_ms() + 3_600_000));
    }
}
