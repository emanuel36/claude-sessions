//! Words for a POSIX shell.

/// Quotes a word for a shell like Python's `shlex.quote`.
///
/// A word made only of characters a shell takes literally is returned as it is; anything else
/// is wrapped in single quotes, each single quote inside spelled `'"'"'`.
pub fn quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c);
    if word.is_empty() {
        "''".to_owned()
    } else if word.chars().all(safe) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r#"'"'"'"#))
    }
}

#[cfg(test)]
mod tests {
    use crate::shell::quote;

    #[test]
    fn quote_matches_shlex_quote() {
        assert_eq!(quote("/opt/bin/cst"), "/opt/bin/cst");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r#"'it'"'"'s'"#);
        assert_eq!(quote("a$b"), "'a$b'");
        assert_eq!(quote("é"), "'é'");
    }
}
