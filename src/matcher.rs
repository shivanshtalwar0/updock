use crate::config::ProjectConfig;

/// Return `true` if `tag` matches `pattern`.
///
/// Pattern rules:
/// - `*`  – matches any single segment (no `/`).
/// - `**` – matches any number of characters including `/`.
/// - All other characters are matched literally (case-sensitive).
pub fn tag_matches(pattern: &str, tag: &str) -> bool {
    // Fast-path: no wildcards.
    if !pattern.contains('*') {
        return pattern == tag;
    }
    glob_match(pattern, tag)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t)
}

fn glob_match_inner(p: &[char], t: &[char]) -> bool {
    match (p.first(), t.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some('*'), _) => {
            // Check for `**`
            if p.get(1) == Some(&'*') {
                // `**` – consume zero or more characters (including `/`).
                //
                // Special case: if `**` is followed by `/` in the pattern,
                // also try matching WITHOUT consuming either `**` or the `/`
                // (i.e., the `**/` prefix is entirely optional, so `**/foo`
                // can match `foo` as well as `a/b/foo`).
                if p.get(2) == Some(&'/') {
                    // Try skipping the `**/` entirely.
                    if glob_match_inner(&p[3..], t) {
                        return true;
                    }
                }
                for i in 0..=t.len() {
                    if glob_match_inner(&p[2..], &t[i..]) {
                        return true;
                    }
                }
                false
            } else {
                // `*` – consume zero or more non-separator characters.
                for i in 0..=t.len() {
                    if t[..i].contains(&'/') {
                        break;
                    }
                    if glob_match_inner(&p[1..], &t[i..]) {
                        return true;
                    }
                }
                false
            }
        }
        (Some(pc), Some(tc)) => {
            if pc == tc {
                glob_match_inner(&p[1..], &t[1..])
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Find every [`ProjectConfig`] that should be triggered by a push to
/// `repository` with tags `pushed_tags`.
pub fn matching_projects<'a>(
    projects: &'a [ProjectConfig],
    repository: &str,
    pushed_tags: &[&str],
) -> Vec<&'a ProjectConfig> {
    projects
        .iter()
        .filter(|p| {
            // Repository name match (case-insensitive).
            if !p.harbor_repository.eq_ignore_ascii_case(repository) {
                return false;
            }

            // Tag filter: if no filter is set, always match.
            match &p.tag_filter {
                None => true,
                Some(filter) => pushed_tags.iter().any(|t| tag_matches(filter, t)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_tag() {
        assert!(tag_matches("latest", "latest"));
        assert!(!tag_matches("latest", "v1.0"));
    }

    #[test]
    fn test_wildcard_star() {
        assert!(tag_matches("v*", "v1.0.0"));
        assert!(tag_matches("v*", "v2"));
        assert!(!tag_matches("v*", "stable"));
        // `*` should not cross `/`
        assert!(!tag_matches("v*", "v1/extra"));
    }

    #[test]
    fn test_wildcard_double_star() {
        assert!(tag_matches("**", "anything"));
        assert!(tag_matches("**", "v1/extra"));
        assert!(tag_matches("prefix/**", "prefix/nested/tag"));
    }
}
