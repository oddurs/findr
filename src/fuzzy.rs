//! Fuzzy matching for the filter: an ordered subsequence match, scored to favour word starts and runs.

/// Scores `text` against `pattern`, returning the score and the matched char positions, or `None`
/// when `pattern` is not a subsequence of `text`. Case-insensitive unless `pattern` has an uppercase letter.
pub fn score(pattern: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    let case_sensitive = pattern.chars().any(char::is_uppercase);
    let fold = |c: char| {
        if case_sensitive {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let pat: Vec<char> = pattern.chars().map(fold).collect();
    let txt: Vec<char> = text.chars().collect();
    let low: Vec<char> = txt.iter().map(|&c| fold(c)).collect();
    let Some(&first) = pat.first() else {
        return Some((0, Vec::new()));
    };

    // Greedy matching from the first occurrence can miss a better alignment later on
    // ("rs" in "r_x_rs"), so try every place the pattern could start.
    let mut best: Option<(i64, Vec<usize>)> = None;
    for start in (0..low.len()).filter(|&i| low[i] == first) {
        // If the rest does not fit after this start it will not fit after a later one either.
        let Some(hits) = greedy(&pat, &low, start) else {
            break;
        };
        let s = rate(&hits, &txt);
        if best.as_ref().is_none_or(|(b, _)| s > *b) {
            best = Some((s, hits));
        }
    }
    best
}

fn greedy(pat: &[char], low: &[char], start: usize) -> Option<Vec<usize>> {
    let mut hits = Vec::with_capacity(pat.len());
    let mut i = start;
    for &p in pat {
        while i < low.len() && low[i] != p {
            i += 1;
        }
        if i == low.len() {
            return None;
        }
        hits.push(i);
        i += 1;
    }
    Some(hits)
}

fn rate(hits: &[usize], txt: &[char]) -> i64 {
    let mut score = 0i64;
    for (n, &i) in hits.iter().enumerate() {
        score += 16;
        if i == 0 || is_boundary(txt[i - 1], txt[i]) {
            score += 12;
        }
        if n > 0 {
            let gap = i - hits[n - 1] - 1;
            score += if gap == 0 { 14 } else { -(gap.min(8) as i64) };
        }
    }
    score - hits[0].min(10) as i64 - txt.len() as i64 / 8
}

fn is_boundary(prev: char, cur: char) -> bool {
    matches!(prev, '_' | '-' | '.' | ' ' | '/') || (prev.is_lowercase() && cur.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(pattern: &str, text: &str) -> Option<Vec<usize>> {
        score(pattern, text).map(|(_, h)| h)
    }

    #[test]
    fn empty_pattern_matches_everything() {
        assert_eq!(score("", "anything"), Some((0, vec![])));
    }

    #[test]
    fn matches_an_ordered_subsequence() {
        assert_eq!(hits("abc", "xaxbxc"), Some(vec![1, 3, 5]));
        assert_eq!(hits("cba", "abc"), None);
        assert_eq!(hits("xyz", "main.rs"), None);
    }

    #[test]
    fn prefers_word_starts() {
        assert_eq!(hits("rs", "r_x_rs"), Some(vec![4, 5]));
        let (a, _) = score("mr", "main.rs").unwrap();
        let (b, _) = score("mr", "semaphore_raw").unwrap();
        assert!(a > b, "main.rs {a} should beat semaphore_raw {b}");
    }

    #[test]
    fn prefers_consecutive_runs() {
        let (run, _) = score("app", "app.rs").unwrap();
        let (spread, _) = score("app", "a_p_p.rs").unwrap();
        assert!(run > spread);
    }

    #[test]
    fn smart_case() {
        assert!(score("m", "Main.rs").is_some());
        assert!(score("M", "main.rs").is_none());
        assert!(score("M", "Main.rs").is_some());
    }
}
