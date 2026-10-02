//! Parsing of what the user types: task references.

/// True for a canonical lowercase or uppercase UUID string.
pub fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Split a display code such as `PROD-4A5B` into the project key and the
/// lowercase id suffix.
pub fn parse_code(value: &str) -> Option<(&str, String)> {
    let (key, suffix) = value.trim().rsplit_once('-')?;
    let key_ok = !key.is_empty()
        && key.len() <= 20
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let suffix_ok = suffix.len() == 4 && suffix.chars().all(|c| c.is_ascii_hexdigit());
    (key_ok && suffix_ok).then(|| (key, suffix.to_ascii_lowercase()))
}

/// Cut `text` to at most `max` characters, ending with `…` when cut.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_uuids_and_codes() {
        assert!(is_uuid("0199a3f2-6b1c-7d24-9e0a-3c5b7d9e4a5b"));
        assert!(!is_uuid("0199a3f2-6b1c-7d24-9e0a-3c5b7d9e4a5"));
        assert!(!is_uuid("PROD-4A5B"));
        assert_eq!(parse_code("prod-4A5B"), Some(("prod", "4a5b".to_owned())));
        assert_eq!(
            parse_code("MY_KEY-00ff"),
            Some(("MY_KEY", "00ff".to_owned()))
        );
        assert_eq!(parse_code("PROD-4A5"), None);
        assert_eq!(parse_code("Fix the login-page"), None);
    }

    #[test]
    fn truncates_by_characters() {
        assert_eq!(truncate("héllo", 5), "héllo");
        assert_eq!(truncate("héllo world", 5), "héll…");
    }
}
