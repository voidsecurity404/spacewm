//! Fuzzy matching for the launcher: "ffx" finds "Firefox", "term" finds "Terminal".
//!
//! The query letters must appear in the name in order (a subsequence, case-insensitive).
//! The score rewards what people mean: a match at the very start, at word starts
//! ("gc" -> "GNOME Calculator"), and runs of consecutive letters; it penalizes gaps.

/// Score of `query` against `name`, or None if it doesn't match. Higher is better.
pub fn score(query: &str, name: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let original: Vec<char> = name.chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (i, &c) in n.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if c != q[qi] {
            continue;
        }
        let word_start = i == 0
            || !n[i - 1].is_alphanumeric()
            // camelCase boundary. `get`: lowercasing can change the length ("İ" -> 2
            // chars), so indices into the original may not line up.
            || (original.get(i).is_some_and(|c| c.is_uppercase())
                && original.get(i - 1).is_some_and(|p| p.is_lowercase()));
        score += 1;
        if i == 0 {
            score += 15;
        } else if word_start {
            score += 8;
        }
        match prev {
            Some(p) if p + 1 == i => score += 5,
            Some(p) => score -= ((i - p - 1) as i32).min(5),
            None => score -= (i as i32).min(5),
        }
        prev = Some(i);
        qi += 1;
    }
    (qi == q.len()).then_some(score)
}

/// Indices of `names` matching `query`, best first (ties: shorter, then alphabetical).
pub fn rank<'a>(query: &str, names: impl Iterator<Item = &'a str>) -> Vec<usize> {
    let mut scored: Vec<(i32, usize, usize, String)> = names
        .enumerate()
        .filter_map(|(i, n)| score(query, n).map(|s| (s, n.len(), i, n.to_lowercase())))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.3.cmp(&b.3)));
    scored.into_iter().map(|(_, _, i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_required() {
        assert!(score("ffx", "Firefox").is_some());
        assert!(score("xff", "Firefox").is_none());
        assert!(score("", "anything").is_some());
        assert!(score("FIRE", "firefox").is_some(), "case-insensitive");
    }

    #[test]
    fn odd_unicode_does_not_panic() {
        assert!(score("i", "İstanbul İzmir").is_some());
        assert!(score("ß", "Straße").is_some());
    }

    #[test]
    fn ranking_prefers_what_people_mean() {
        let names = ["Text Editor", "Firefox", "Fonts", "Files", "LibreOffice Draw"];
        let r = rank("fi", names.iter().copied());
        assert_eq!(names[r[0]], "Files", "prefix + shortest wins: {:?}", r);
        assert!(r.contains(&1));
        let r = rank("ffx", names.iter().copied());
        assert_eq!(names[r[0]], "Firefox");
        let r = rank("te", names.iter().copied());
        assert_eq!(names[r[0]], "Text Editor");
        // Word starts: "gc" -> GNOME Calculator before a name with g...c in the middle.
        let names = ["Magic Tool", "GNOME Calculator"];
        assert_eq!(names[rank("gc", names.iter().copied())[0]], "GNOME Calculator");
    }
}
