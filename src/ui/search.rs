//! Finding sessions of any folder by their title and the prompts typed in them.

/// A session the search looks in, from any folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Searchable {
    pub id: String,
    pub title: String,
    /// The folder's path, no trailing slash.
    pub path: String,
    /// The folder's last path component.
    pub name: String,
    pub last: f64,
    /// Whether the folder no longer exists.
    pub gone: bool,
    /// The title, lowercased.
    title_lower: String,
    /// The title when the transcript gave it, and every prompt, lowercased, joined by `\n`.
    text_lower: String,
}

impl Searchable {
    /// Builds a searchable session.
    ///
    /// # Arguments
    ///
    /// * `own_title` - whether `title` came from the transcript rather than the first prompt;
    ///   only then is it added to the searched text, so a first prompt is not counted twice.
    /// * `prompts` - every prompt typed in the session.
    pub fn new(
        id: String,
        title: String,
        path: String,
        last: f64,
        gone: bool,
        own_title: bool,
        prompts: &[String],
    ) -> Self {
        // A first prompt title is the raw prompt, placeholders included; `prompts` has none
        let title_lower = if own_title {
            title.to_lowercase()
        } else {
            prompts
                .first()
                .map_or_else(String::new, |p| p.to_lowercase())
        };
        let mut parts: Vec<&str> = Vec::with_capacity(prompts.len() + 1);
        if own_title {
            parts.push(&title_lower);
        }
        let lowered: Vec<String> = prompts.iter().map(|p| p.to_lowercase()).collect();
        parts.extend(lowered.iter().map(String::as_str));
        let text_lower = parts.join("\n");
        let name = name(&path);
        Self {
            id,
            title,
            path,
            name,
            last,
            gone,
            title_lower,
            text_lower,
        }
    }
}

/// A match: the session's index in the searched slice and how often the words occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub index: usize,
    pub count: usize,
}

/// A folder's name: its last path component, or the whole path when it has none.
pub fn name(path: &str) -> String {
    match path.rsplit('/').next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => path.to_owned(),
    }
}

/// The sessions holding every word of `query`, ignoring case, ranked: title holding every word
/// first, then newest first. Empty when `query` is blank.
///
/// `count` sums, over the words, the non-overlapping occurrences in the searched text.
pub fn hits(sessions: &[Searchable], query: &str) -> Vec<Hit> {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<(bool, Hit)> = Vec::new();
    for (index, s) in sessions.iter().enumerate() {
        if !words
            .iter()
            .all(|w| s.text_lower.contains(w) || s.title_lower.contains(w))
        {
            continue;
        }
        let in_title = words.iter().all(|w| s.title_lower.contains(w));
        let count = words.iter().map(|w| s.text_lower.matches(w).count()).sum();
        found.push((in_title, Hit { index, count }));
    }
    found.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            sessions[b.1.index]
                .last
                .total_cmp(&sessions[a.1.index].last)
        })
    });
    found.into_iter().map(|(_, hit)| hit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn searchable(id: &str, title: &str, last: f64, prompts: &[&str]) -> Searchable {
        let prompts: Vec<String> = prompts.iter().map(|p| (*p).to_owned()).collect();
        Searchable::new(
            id.into(),
            title.into(),
            "/p".into(),
            last,
            false,
            true,
            &prompts,
        )
    }

    #[test]
    fn words_may_sit_in_different_prompts() {
        let s = [searchable(
            "a",
            "Banner",
            10.0,
            &["about LinkedIn", "em inglês"],
        )];
        assert_eq!(
            hits(&s, "linkedin INGLÊS"),
            vec![Hit { index: 0, count: 2 }]
        );
    }

    #[test]
    fn every_word_must_occur() {
        let s = [searchable("a", "Banner", 10.0, &["about LinkedIn"])];
        assert!(hits(&s, "linkedin python").is_empty());
    }

    #[test]
    fn title_matches_come_first_then_the_newest() {
        let s = [
            searchable("old", "x", 1.0, &["linkedin"]),
            searchable("new", "x", 3.0, &["linkedin"]),
            searchable("titled", "LinkedIn post", 2.0, &["hello"]),
        ];
        let order: Vec<usize> = hits(&s, "linkedin").iter().map(|h| h.index).collect();
        assert_eq!(order, vec![2, 1, 0]);
    }

    #[test]
    fn a_first_prompt_title_is_not_counted_twice() {
        let s = [Searchable::new(
            "a".into(),
            "linkedin".into(),
            "/p".into(),
            1.0,
            false,
            false,
            &["linkedin".to_owned()],
        )];
        assert_eq!(hits(&s, "linkedin")[0].count, 1);
    }

    fn untitled(id: &str, first_prompt: &str, prompts: &[&str]) -> Searchable {
        let prompts: Vec<String> = prompts.iter().map(|p| (*p).to_owned()).collect();
        let title = first_prompt.to_owned();
        Searchable::new(id.into(), title, "/p".into(), 1.0, false, false, &prompts)
    }

    #[test]
    fn a_placeholder_first_prompt_is_not_a_title_match() {
        let s = [untitled("a", "[Pasted text #1 +20 lines]", &[])];
        assert!(hits(&s, "pasted").is_empty());
    }

    #[test]
    fn a_first_prompt_title_ignores_placeholders_but_still_ranks() {
        let s = [
            untitled("old", "x", &["x", "linkedin"]),
            untitled("new", "[Image #1] LinkedIn banner", &["LinkedIn banner"]),
        ];
        let mut s = s;
        s[0].last = 9.0;
        let order: Vec<usize> = hits(&s, "linkedin").iter().map(|h| h.index).collect();
        assert_eq!(order, vec![1, 0]);
    }

    #[test]
    fn blank_text_finds_nothing() {
        let s = [searchable("a", "x", 1.0, &["y"])];
        assert!(hits(&s, "   ").is_empty());
    }

    #[test]
    fn the_name_is_the_last_component_or_the_whole_path() {
        assert_eq!(name("/a/b"), "b");
        assert_eq!(name("/"), "/");
    }
}
