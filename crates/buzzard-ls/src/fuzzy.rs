//! Small, allocation-light fuzzy matcher for workspace symbol search.

/// Score `candidate` against `query` (case-insensitive subsequence match).
/// Returns `None` when the query does not match. Higher is better.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let mut total = 0;
    let mut wanted = query.chars().map(|c| c.to_ascii_lowercase()).peekable();
    let mut previous_matched = false;
    let mut previous: Option<char> = None;
    for (i, c) in candidate.chars().enumerate() {
        let Some(&want) = wanted.peek() else { break };
        if c.to_ascii_lowercase() == want {
            wanted.next();
            total += 10;
            if previous_matched {
                total += 15;
            }
            let boundary = match previous {
                None => true,
                Some(p) => !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()),
            };
            if boundary {
                total += 20;
            }
            if i == 0 {
                total += 10;
            }
            previous_matched = true;
        } else {
            previous_matched = false;
        }
        previous = Some(c);
    }
    if wanted.peek().is_some() {
        return None;
    }
    Some(total - candidate.chars().count() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequences_case_insensitively() {
        assert!(score("rdcfg", "Read Config.vi").is_some());
        assert!(score("READ", "read config.vi").is_some());
        assert!(score("xyz", "Read Config.vi").is_none());
        assert!(score("configg", "Read Config.vi").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn ranks_better_matches_higher() {
        let s = |candidate| score("init", candidate).unwrap();
        assert!(s("Init.vi") > s("Reinitialize.vi"));
        assert!(s("Init.vi") > s("Init Hardware And Wait.vi"));
        assert!(s("Motor Init.vi") > s("Main Unit Test.vi"));
    }
}
