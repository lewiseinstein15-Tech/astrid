//! Hybrid retrieval: BM25-lite + recency decay + exact-phrase, fused with a
//! deterministic Reciprocal Rank Fusion (RRF, k=60).
//!
//! Three stages produce independent rankings over the non-forgotten pages:
//!
//! 1. **bm25** — classic Okapi BM25 over title+body tokens (title counted
//!    twice), lowercase alnum tokenization, `idf = ln(1 + N/df)`, tf
//!    saturation with `k1 = 1.2`, `b = 0.75`.
//! 2. **recency** — pages decay by days-since-updated with a 14-day
//!    half-life; ranks every candidate (this is the newest-k fallback path).
//! 3. **phrase** — the full query as a case-insensitive substring of title
//!    or body; binary 1.0/0.0.
//!
//! The fused score is `Σ_stages 1/(60 + rank)` over the stage rankings, so
//! results are fully deterministic: ties break by updated desc, then id asc.

use crate::protocol::RecallHit;
use crate::store::Page;
use chrono::Utc;

/// BM25 term-frequency saturation constant.
pub const BM25_K1: f64 = 1.2;
/// BM25 length-normalization constant.
pub const BM25_B: f64 = 0.75;
/// Recency decay half-life in days.
pub const RECENCY_HALF_LIFE_DAYS: f64 = 14.0;
/// Reciprocal Rank Fusion constant.
pub const RRF_K: f64 = 60.0;
/// Maximum snippet length in characters.
pub const SNIPPET_CHARS: usize = 160;

/// Lowercase alnum tokenization (unicode-aware alphanumeric runs).
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whole days since `updated` (an RFC 3339 timestamp). Unparsable
/// timestamps are treated as very old (10 years) rather than fatal.
pub fn days_since(updated: &str) -> f64 {
    match chrono::DateTime::parse_from_rfc3339(updated) {
        Ok(dt) => {
            let days = (Utc::now() - dt.with_timezone(&Utc)).num_days();
            if days < 0 {
                0.0
            } else {
                days as f64
            }
        }
        Err(_) => 3650.0,
    }
}

/// Recency boost: exponential decay with a 14-day half-life.
pub fn recency_score(updated: &str) -> f64 {
    0.5f64.powf(days_since(updated) / RECENCY_HALF_LIFE_DAYS)
}

/// Exact-phrase stage: 1.0 if the trimmed query appears (case-insensitive)
/// in the title or body, else 0.0.
pub fn phrase_score(query: &str, page: &Page) -> f64 {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return 0.0;
    }
    if page.title.to_lowercase().contains(&q) || page.body.to_lowercase().contains(&q) {
        1.0
    } else {
        0.0
    }
}

/// BM25-lite scores for every page against the query tokens.
pub fn bm25_scores(query_tokens: &[String], pages: &[&Page]) -> Vec<f64> {
    let n = pages.len();
    let mut scores = vec![0.0; n];
    if n == 0 {
        return scores;
    }
    let unique_tokens: Vec<&String> = {
        let mut seen = std::collections::BTreeSet::new();
        query_tokens
            .iter()
            .filter(|t| seen.insert(t.as_str()))
            .collect()
    };
    if unique_tokens.is_empty() {
        return scores;
    }

    // Document tokens: title counted twice (title matches weigh more than
    // body matches), plus the body.
    let docs: Vec<Vec<String>> = pages
        .iter()
        .map(|p| {
            let mut tokens = tokenize(&p.title);
            tokens.extend(tokenize(&p.title));
            tokens.extend(tokenize(&p.body));
            tokens
        })
        .collect();

    let total_len: usize = docs.iter().map(|d| d.len()).sum();
    let avg_len = (total_len as f64 / n as f64).max(1.0);

    // Document frequency per unique query token.
    let df: std::collections::BTreeMap<&str, usize> = unique_tokens
        .iter()
        .map(|t| {
            let count = docs
                .iter()
                .filter(|doc| doc.iter().any(|tok| tok == *t))
                .count();
            (t.as_str(), count)
        })
        .collect();

    for (i, doc) in docs.iter().enumerate() {
        let len = (doc.len() as f64).max(1.0);
        let norm = BM25_K1 * (1.0 - BM25_B + BM25_B * len / avg_len);
        let mut score = 0.0;
        for t in &unique_tokens {
            let tf = doc.iter().filter(|tok| *tok == *t).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let df = df.get(t.as_str()).copied().unwrap_or(0);
            let idf = (1.0 + n as f64 / df.max(1) as f64).ln();
            score += idf * (tf * (BM25_K1 + 1.0)) / (tf + norm);
        }
        scores[i] = score;
    }
    scores
}

/// Deterministic stage ranking over candidate indices: score desc, then
/// updated desc, then id asc. When `nonzero_only`, zero-scored candidates
/// are excluded from the ranking entirely.
fn rank_order(candidates: &[&Page], scores: &[f64], nonzero_only: bool) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..candidates.len())
        .filter(|&i| !nonzero_only || scores[i] > 0.0)
        .collect();
    idx.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(candidates[b].updated.cmp(&candidates[a].updated))
            .then(candidates[a].id.cmp(&candidates[b].id))
    });
    idx
}

/// Case-insensitive char-boundary-safe snippet around the first query-token
/// match; falls back to the start of the body.
pub fn snippet(body: &str, query_tokens: &[String], max_chars: usize) -> String {
    let lower = body.to_lowercase();
    let mut match_pos: Option<usize> = None;
    for token in query_tokens {
        if let Some(pos) = lower.find(token.as_str()) {
            match_pos = Some(match match_pos {
                Some(existing) => existing.min(pos),
                None => pos,
            });
        }
    }
    let char_safe = |mut i: usize| -> usize {
        while i > 0 && !body.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let end_clamp = |mut i: usize| -> usize {
        while i < body.len() && !body.is_char_boundary(i) {
            i += 1;
        }
        i
    };
    let (start, end) = match match_pos {
        Some(pos) => {
            let half = max_chars / 2;
            let start = char_safe(pos.saturating_sub(half));
            let end = end_clamp((start + max_chars).min(body.len()));
            (start, end)
        }
        None => (0, end_clamp(max_chars.min(body.len()))),
    };
    body[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run the three-stage hybrid search and fuse with RRF. Forgotten pages are
/// excluded. An empty (or tokenless) query degrades gracefully to the
/// recency-only ranking — i.e. the newest pages first.
pub fn search(query: &str, pages: &[Page], k: usize) -> Vec<RecallHit> {
    if k == 0 {
        return Vec::new();
    }
    let candidates: Vec<&Page> = pages.iter().filter(|p| !p.forgotten).collect();
    if candidates.is_empty() {
        return Vec::new();
    }

    let query_tokens = tokenize(query);
    let bm25 = bm25_scores(&query_tokens, &candidates);
    let recency: Vec<f64> = candidates
        .iter()
        .map(|p| recency_score(&p.updated))
        .collect();
    let phrase: Vec<f64> = candidates.iter().map(|p| phrase_score(query, p)).collect();

    // RRF fusion over the three stage rankings. The recency stage ranks
    // every candidate (so a tokenless query still returns newest-k);
    // bm25 and phrase contribute only when they have something to say.
    let mut fused = vec![0.0f64; candidates.len()];
    let stages: [(&str, &[f64], bool); 3] = [
        ("bm25", &bm25, true),
        ("recency", &recency, false),
        ("phrase", &phrase, true),
    ];
    for (_, scores, nonzero_only) in stages {
        for (rank_zero_based, &idx) in rank_order(&candidates, scores, nonzero_only)
            .iter()
            .enumerate()
        {
            fused[idx] += 1.0 / (RRF_K + rank_zero_based as f64 + 1.0);
        }
    }

    let order = rank_order(&candidates, &fused, false);
    order
        .into_iter()
        .take(k)
        .map(|i| {
            let page = candidates[i];
            RecallHit {
                id: page.id.clone(),
                title: page.title.clone(),
                snippet: snippet(&page.body, &query_tokens, SNIPPET_CHARS),
                score: fused[i],
                stages: vec![
                    ("bm25".to_string(), bm25[i]),
                    ("recency".to_string(), recency[i]),
                    ("phrase".to_string(), phrase[i]),
                ],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str, title: &str, body: &str, days_old: f64) -> Page {
        let updated = (Utc::now() - chrono::Duration::days(days_old as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Page {
            id: format!("page_{id}"),
            title: title.to_string(),
            kind: "note".into(),
            created: updated.clone(),
            updated,
            forgotten: false,
            aliases: vec![],
            edges: vec![],
            sources: vec![],
            forgotten_reason: None,
            forgotten_at: None,
            body: body.to_string(),
            path: format!("notes/{id}.md"),
        }
    }

    #[test]
    fn tokenizer_is_lowercase_alnum() {
        assert_eq!(
            tokenize("Hello, World! Ünïcode_42 3x"),
            vec!["hello", "world", "ünïcode", "42", "3x"]
        );
        assert!(tokenize("--- !!!").is_empty());
    }

    #[test]
    fn recency_decays_with_half_life() {
        let now = recency_score(&Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        assert!((now - 1.0).abs() < 1e-9);
        assert!(
            (recency_score(
                &(Utc::now() - chrono::Duration::days(14))
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            ) - 0.5)
                .abs()
                < 0.01
        );
        assert_eq!(days_since("not a timestamp"), 3650.0);
    }

    #[test]
    fn phrase_stage_is_case_insensitive_substring() {
        let p = page("p", "T", "The Quick BROWN fox", 0.0);
        assert_eq!(phrase_score("quick brown", &p), 1.0);
        assert_eq!(phrase_score("  QUICK BROWN ", &p), 1.0);
        assert_eq!(phrase_score("slow brown", &p), 0.0);
        assert_eq!(phrase_score("   ", &p), 0.0);
    }

    #[test]
    fn empty_query_falls_back_to_newest_first() {
        let pages = vec![
            page("old", "Old page", "nothing relevant here", 60.0),
            page("new", "New page", "also nothing relevant", 0.0),
            page("mid", "Mid page", "still nothing", 7.0),
        ];
        let hits = search("", &pages, 10);
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["page_new", "page_mid", "page_old"]);
        // and a tokenless query behaves the same way
        let hits = search("!!! ---", &pages, 10);
        assert_eq!(hits[0].id, "page_new");
    }

    #[test]
    fn forgotten_pages_are_never_recalled() {
        let mut forgotten = page("f", "Forgotten", "find me", 0.0);
        forgotten.forgotten = true;
        let pages = vec![forgotten, page("k", "Keeper", "find me too", 0.0)];
        let hits = search("find me", &pages, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "page_k");
    }

    #[test]
    fn snippet_centers_on_first_match() {
        let body = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua";
        let s = snippet(body, &["tempor".to_string()], 40);
        assert!(s.contains("tempor"));
        assert!(s.len() <= 40 + "tempor".len() + 2); // + slack for whitespace collapse
                                                     // no match: prefix of the body
        let s = snippet(body, &["zzz".to_string()], 20);
        assert!(s.starts_with("lorem"));
    }
}
