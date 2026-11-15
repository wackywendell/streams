//! Checked desktop URLs and the existing terminal card's web-link policy.

use percent_encoding::percent_decode_str;
use unicode_general_category::{GeneralCategory, get_general_category};
use url::Url;

/// A credential-free absolute HTTP(S) URL, safe to hand to a desktop viewer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SafeHttpUrl(Url);

impl SafeHttpUrl {
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}
impl std::str::FromStr for SafeHttpUrl {
    type Err = crate::model::ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || {
            crate::model::ValidationError::new("expected a credential-free absolute HTTP(S) URL")
        };
        let url = parse_web_url(value).ok_or_else(invalid)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(invalid());
        }
        Ok(Self(url))
    }
}
impl TryFrom<String> for SafeHttpUrl {
    type Error = crate::model::ValidationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}
impl From<SafeHttpUrl> for String {
    fn from(value: SafeHttpUrl) -> Self {
        value.0.into()
    }
}

// Keep the terminal card's existing URL behavior; native views additionally reject credentials.
pub(crate) fn parse_web_url(value: &str) -> Option<Url> {
    if value.chars().any(|c| {
        c.is_whitespace()
            || c == '\\'
            || matches!(
                get_general_category(c),
                GeneralCategory::Control | GeneralCategory::Format
            )
    }) {
        return None;
    }
    // decodeURIComponent rejects malformed escapes and invalid UTF-8; percent_decode
    // deliberately does not reject malformed escapes, so check the escape shape first.
    let bytes = value.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'%'
            && !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    let decoded = percent_decode_str(value).decode_utf8().ok()?;
    if decoded.chars().any(|c| {
        matches!(
            get_general_category(c),
            GeneralCategory::Control | GeneralCategory::Format
        )
    }) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_urls_reject_credentials_nonweb_targets_and_hidden_control_characters() {
        for value in [
            "/relative",
            "file:///tmp/plan.md",
            "javascript:alert(1)",
            "https://",
            "https://user@example.test/",
            "https://user:password@example.test/",
            "https://example.test/with space",
            "https://example.test/\\\\path",
            "https://example.test/%0a",
            "https://example.test/%zz",
            "https://example.test/\u{200b}",
        ] {
            assert!(value.parse::<SafeHttpUrl>().is_err(), "{value:?}");
        }
        let value: SafeHttpUrl = "https://example.test/issue/1?q=two#details"
            .parse()
            .unwrap();
        assert_eq!(
            value.as_url().as_str(),
            "https://example.test/issue/1?q=two#details"
        );
        let encoded = serde_json::to_value(&value).unwrap();
        assert_eq!(
            serde_json::from_value::<SafeHttpUrl>(encoded).unwrap(),
            value
        );
        assert!(
            serde_json::from_value::<SafeHttpUrl>(
                serde_json::json!({"url":"https://example.test"})
            )
            .is_err()
        );
    }

    #[test]
    fn extraction_preserves_terminal_card_credential_policy() {
        assert!(parse_web_url("https://user@example.test/issue/1").is_some());
    }
}
