use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

use crate::config::SearchWeights;

// ── Public result types ──────────────────────────────────────────────────────

pub enum ResultKind {
    GhIssue,
    Doc,
    DiscordThread,
}

pub struct SearchResult {
    pub kind: ResultKind,
    pub title: String,
    pub url: Option<String>,
    pub repo: Option<String>,
    pub score: f64,
    pub updated_at: Option<DateTime<Utc>>,
}

// ── Internal candidate (result + raw text for scoring) ───────────────────────

struct Candidate {
    result: SearchResult,
    title: String,
    body: String,
    comments: String,
    /// Per-entry score multiplier from config.
    source_weight: f64,
}

// ── Text helpers ─────────────────────────────────────────────────────────────

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Raw term counts for a field.
fn count_terms(tokens: &[String]) -> HashMap<String, f64> {
    let mut counts: HashMap<String, f64> = HashMap::new();
    for t in tokens {
        *counts.entry(t.clone()).or_insert(0.0) += 1.0;
    }
    counts
}

/// Return the source-specific corpus bucket for a candidate.
fn source_bucket(kind: &ResultKind) -> usize {
    match kind {
        ResultKind::GhIssue => 0,
        ResultKind::Doc => 1,
        ResultKind::DiscordThread => 2,
    }
}

#[derive(Clone)]
struct CandidateTokens {
    title: Vec<String>,
    body: Vec<String>,
    comments: Vec<String>,
}

struct Bm25Stats {
    document_count: f64,
    average_title_len: f64,
    average_body_len: f64,
    average_comment_len: f64,
    document_frequency: HashMap<String, f64>,
}

/// Build BM25 document-frequency and field-length statistics independently
/// for each source type.
fn build_source_bm25_stats(candidates: &[CandidateTokens], buckets: &[usize]) -> [Bm25Stats; 3] {
    let mut grouped: [Vec<&CandidateTokens>; 3] = std::array::from_fn(|_| Vec::new());
    for (candidate, bucket) in candidates.iter().zip(buckets) {
        grouped[*bucket].push(candidate);
    }

    grouped.map(|group| {
        let document_count = group.len() as f64;
        let mut document_frequency = HashMap::new();
        let mut title_len = 0usize;
        let mut body_len = 0usize;
        let mut comment_len = 0usize;

        for candidate in &group {
            title_len += candidate.title.len();
            body_len += candidate.body.len();
            comment_len += candidate.comments.len();

            let unique_terms: std::collections::HashSet<&String> = candidate
                .title
                .iter()
                .chain(candidate.body.iter())
                .chain(candidate.comments.iter())
                .collect();
            for term in unique_terms {
                *document_frequency.entry(term.clone()).or_insert(0.0) += 1.0;
            }
        }

        Bm25Stats {
            document_count,
            average_title_len: title_len as f64 / document_count.max(1.0),
            average_body_len: body_len as f64 / document_count.max(1.0),
            average_comment_len: comment_len as f64 / document_count.max(1.0),
            document_frequency,
        }
    })
}

fn normalized_term_frequency(count: f64, field_len: usize, average_field_len: f64, b: f64) -> f64 {
    if count == 0.0 {
        return 0.0;
    }
    count / (1.0 - b + b * field_len as f64 / average_field_len.max(1.0))
}

/// Field-weighted BM25 (BM25F). Title, body, and comment matches are
/// normalised independently before their configured weights are combined.
fn bm25_score(
    query_tokens: &[String],
    candidate: &CandidateTokens,
    stats: &Bm25Stats,
    weights: &SearchWeights,
) -> f64 {
    if stats.document_count == 0.0 {
        return 0.0;
    }

    let title_counts = count_terms(&candidate.title);
    let body_counts = count_terms(&candidate.body);
    let comment_counts = count_terms(&candidate.comments);
    let query_terms: std::collections::HashSet<&String> = query_tokens.iter().collect();

    query_terms
        .into_iter()
        .map(|term| {
            let df = stats.document_frequency.get(term).copied().unwrap_or(0.0);
            let idf = ((stats.document_count - df + 0.5) / (df + 0.5)).ln_1p();
            let field_tf = weights.title_weight
                * normalized_term_frequency(
                    title_counts.get(term).copied().unwrap_or(0.0),
                    candidate.title.len(),
                    stats.average_title_len,
                    weights.bm25_b,
                )
                + weights.body_weight
                    * normalized_term_frequency(
                        body_counts.get(term).copied().unwrap_or(0.0),
                        candidate.body.len(),
                        stats.average_body_len,
                        weights.bm25_b,
                    )
                + weights.comment_weight
                    * normalized_term_frequency(
                        comment_counts.get(term).copied().unwrap_or(0.0),
                        candidate.comments.len(),
                        stats.average_comment_len,
                        weights.bm25_b,
                    );

            idf * (field_tf * (weights.bm25_k1 + 1.0) / (field_tf + weights.bm25_k1))
        })
        .sum()
}

// ── Version weight ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

fn parse_version(tag: &str) -> Option<Version> {
    let s = tag.trim().strip_prefix('v').unwrap_or(tag.trim());
    let main_part = s.split('-').next()?.split('+').next()?;
    let parts: Vec<&str> = main_part.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;
    let patch = parts[2].parse::<u64>().ok()?;
    Some(Version {
        major,
        minor,
        patch,
    })
}

struct ReleaseEntry {
    timestamp: DateTime<Utc>,
    version: Option<Version>,
}

struct ReleaseIndex {
    /// Release publish timestamps and versions sorted ascending by timestamp.
    entries: Vec<ReleaseEntry>,
}

impl ReleaseIndex {
    fn build(mut entries: Vec<ReleaseEntry>) -> Self {
        entries.dedup_by_key(|e| e.timestamp);
        entries.sort_by_key(|e| e.timestamp);
        Self { entries }
    }

    fn closest_idx(&self, ts: DateTime<Utc>) -> usize {
        if self.entries.is_empty() {
            return 0;
        }
        match self.entries.binary_search_by_key(&ts, |e| e.timestamp) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) if i >= self.entries.len() => self.entries.len() - 1,
            Err(i) => {
                let before = (ts - self.entries[i - 1].timestamp).abs().num_seconds();
                let after = (self.entries[i].timestamp - ts).abs().num_seconds();
                if before <= after { i - 1 } else { i }
            }
        }
    }

    /// Returns a weight in (0, 1] that penalises a candidate whose closest
    /// release differs from the query's closest release.
    ///
    /// Formula:
    /// ```text
    /// semantic_distance = major_diff × version_major_penalty
    ///                   + minor_diff × version_minor_penalty
    ///                   + patch_diff × version_patch_penalty
    ///
    /// version_weight    = 1 / (1 + semantic_distance × version_distance_scale)
    /// ```
    /// Same release → distance 0 → weight 1.0.
    ///
    /// **Fallback** (when versions cannot be parsed): uses normalised
    /// release-index offset so the scale is index-size-independent:
    /// ```text
    /// normalised_distance = index_diff / max(1, index_size − 1)
    /// version_weight      = 1 / (1 + normalised_distance × version_index_distance_scale)
    /// ```
    fn version_weight(
        &self,
        query_ts: DateTime<Utc>,
        candidate_ts: DateTime<Utc>,
        weights: &SearchWeights,
    ) -> f64 {
        if self.entries.is_empty() {
            return 1.0;
        }

        let query_idx = self.closest_idx(query_ts);
        let candidate_idx = self.closest_idx(candidate_ts);

        let query_ver = self.entries[query_idx].version;
        let candidate_ver = self.entries[candidate_idx].version;

        let semantic_distance = match (query_ver, candidate_ver) {
            (Some(q), Some(c)) => {
                let major_diff = (q.major as i64 - c.major as i64).unsigned_abs() as f64;
                let minor_diff = (q.minor as i64 - c.minor as i64).unsigned_abs() as f64;
                let patch_diff = (q.patch as i64 - c.patch as i64).unsigned_abs() as f64;

                major_diff * weights.version_major_penalty
                    + minor_diff * weights.version_minor_penalty
                    + patch_diff * weights.version_patch_penalty
            }
            _ => {
                // No parsed versions: normalise by index size so that one slot
                // of distance has a consistent meaning regardless of how many
                // releases exist. (index_diff / max_possible_diff)
                let index_diff = (query_idx as i64 - candidate_idx as i64).unsigned_abs() as f64;
                let max_possible = (self.entries.len() as f64 - 1.0).max(1.0);
                index_diff / max_possible
            }
        };

        let scale = match (query_ver, candidate_ver) {
            (Some(_), Some(_)) => weights.version_distance_scale,
            _ => weights.version_index_distance_scale,
        };

        1.0 / (1.0 + semantic_distance * scale)
    }
}

// ── Candidate loaders ────────────────────────────────────────────────────────

async fn load_gh_issues(db: &SqlitePool, issue_weights: &HashMap<String, f64>) -> Vec<Candidate> {
    let rows = sqlx::query(
        "SELECT i.repo, i.number, i.title, i.updated_at,
                COALESCE(i.body, '') AS body,
                COALESCE(GROUP_CONCAT(c.body, ' '), '') AS comments
         FROM gh_issues i
         LEFT JOIN gh_comments c ON c.issue_id = i.id
         WHERE i.is_pr = 0
         GROUP BY i.id",
    )
    .fetch_all(db)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|row| {
            let repo: String = row.try_get("repo").ok()?;
            let number: i64 = row.try_get("number").ok()?;
            let title: String = row.try_get("title").ok()?;
            let updated_at_str: Option<String> = row.try_get("updated_at").ok();
            let body: String = row.try_get("body").ok().unwrap_or_default();
            let comments: String = row.try_get("comments").ok().unwrap_or_default();

            let updated_at = parse_rfc3339(updated_at_str.as_deref());
            let url = format!("https://github.com/{}/issues/{}", repo, number);
            let source_weight = issue_weights.get(&repo).copied().unwrap_or(1.0);

            Some(Candidate {
                result: SearchResult {
                    kind: ResultKind::GhIssue,
                    title: format!("#{}: {}", number, title),
                    url: Some(url),
                    repo: Some(repo),
                    score: 0.0,
                    updated_at,
                },
                title,
                body,
                comments,
                source_weight,
            })
        })
        .collect()
}

async fn load_docs(db: &SqlitePool, doc_weights: &HashMap<String, f64>) -> Vec<Candidate> {
    let rows = sqlx::query("SELECT repo, file_path, url, title, content, updated_at FROM docs")
        .fetch_all(db)
        .await
        .unwrap_or_default();

    rows.into_iter()
        .filter_map(|row| {
            let repo: String = row.try_get("repo").ok()?;
            let file_path: String = row.try_get("file_path").ok()?;
            let url: Option<String> = row.try_get("url").ok();
            let db_title: Option<String> =
                row.try_get("title").ok().filter(|t: &String| !t.is_empty());
            let content: String = row.try_get("content").ok()?;
            let updated_at_str: Option<String> = row.try_get("updated_at").ok();
            let source_weight: f64 = doc_weights.get(&repo).copied().unwrap_or(1.0);

            let updated_at = parse_rfc3339(updated_at_str.as_deref());
            // Prefer DB title (_meta.json) → markdown heading → filename stem
            let title = db_title.unwrap_or_else(|| filename_stem(&file_path).to_string());
            Some(Candidate {
                result: SearchResult {
                    kind: ResultKind::Doc,
                    title: title.clone(),
                    url,
                    repo: Some(repo),
                    score: 0.0,
                    updated_at,
                },
                title,
                body: content,
                comments: String::new(),
                source_weight,
            })
        })
        .collect()
}

async fn load_threads(
    db: &SqlitePool,
    guild_id: u64,
    forum_weights: &HashMap<String, f64>,
) -> Vec<Candidate> {
    let guild_id_str = guild_id.to_string();

    let rows = sqlx::query(
        "SELECT t.id, t.guild_id, t.forum_channel_id, COALESCE(t.name, '') AS name,
                COALESCE(GROUP_CONCAT(m.content, ' '), '') AS messages_text
         FROM threads t
         LEFT JOIN messages m ON m.thread_id = t.id
         WHERE t.guild_id = ?
         GROUP BY t.id",
    )
    .bind(&guild_id_str)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|row| {
            let id: String = row.try_get("id").ok()?;
            let guild_id_col: String = row.try_get("guild_id").ok()?;
            let forum_channel_id: String = row.try_get("forum_channel_id").ok().unwrap_or_default();
            let name: String = row.try_get("name").ok().unwrap_or_default();
            let messages_text: String = row.try_get("messages_text").ok().unwrap_or_default();

            let title = if name.is_empty() {
                format!("Thread {}", &id[..id.len().min(8)])
            } else {
                name
            };

            let url = format!("https://discord.com/channels/{}/{}", guild_id_col, id);
            let source_weight = forum_weights.get(&forum_channel_id).copied().unwrap_or(1.0);

            Some(Candidate {
                result: SearchResult {
                    kind: ResultKind::DiscordThread,
                    title: title.clone(),
                    url: Some(url),
                    repo: None,
                    score: 0.0,
                    updated_at: None,
                },
                title,
                body: messages_text,
                comments: String::new(),
                source_weight,
            })
        })
        .collect()
}

async fn load_release_index(db: &SqlitePool) -> ReleaseIndex {
    let rows =
        sqlx::query("SELECT tag_name, published_at FROM gh_releases WHERE published_at != ''")
            .fetch_all(db)
            .await
            .unwrap_or_default();

    let entries: Vec<ReleaseEntry> = rows
        .into_iter()
        .filter_map(|row| {
            let tag: String = row.try_get("tag_name").ok()?;
            let s: String = row.try_get("published_at").ok()?;
            let timestamp = parse_rfc3339(Some(&s))?;
            let version = parse_version(&tag);
            Some(ReleaseEntry { timestamp, version })
        })
        .collect();

    ReleaseIndex::build(entries)
}

// ── String helpers ───────────────────────────────────────────────────────────

fn parse_rfc3339(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Return the filename stem from a path (e.g. `docs/foo.mdx` → `foo`).
fn filename_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.find('.').map(|i| &name[..i]).unwrap_or(name)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Find the most relevant DB entries for `query`, scoped to `guild_id` for
/// Discord threads. Returns up to `max_results` results ordered by score.
///
/// Scoring formula (applied in two passes):
///
/// **Pass 1 — base score** (per candidate):
/// ```text
/// relevance    = field-weighted BM25(query, candidate)
/// version_wt   = 1 / (1 + semantic_distance × version_distance_scale)
/// source_wt    = per-entry weight from data_repositories config         -- (default 1.0)
///
/// base_score   = relevance × version_wt × source_wt
/// ```
///
/// **Pass 2 — recency blend** (after filtering):
/// ```text
/// recency      = (candidate_updated_at − oldest_updated_at) / time_span   -- [0, 1]
/// recency_mult = recency_base + recency_influence × recency               -- [0.8, 1.0]
///
/// final_score  = base_score × recency_mult
/// ```
pub async fn find_similar(
    query: &str,
    guild_id: u64,
    db: &SqlitePool,
    threshold: f64,
    max_results: usize,
    weights: &SearchWeights,
    repos: &crate::config::DataRepositories,
) -> Vec<SearchResult> {
    // Build weight lookup maps from config so each loader can annotate candidates.
    let issue_weights: HashMap<String, f64> = repos
        .github_issues
        .iter()
        .map(|s| (s.github_repo.clone(), s.weight))
        .collect();
    // Doc repo names are derived from the clone URL the same way git.rs does it.
    let doc_weights: HashMap<String, f64> = repos
        .git_docs
        .iter()
        .map(|s| {
            let repo_name = s
                .repository
                .trim_end_matches(".git")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_string();
            (repo_name, s.weight)
        })
        .collect();
    let forum_weights: HashMap<String, f64> = repos
        .discord_forums
        .iter()
        .map(|f| (f.channel_id.clone(), f.weight))
        .collect();

    // Load all candidate sources concurrently.
    let (issues, docs, threads, release_index) = tokio::join!(
        load_gh_issues(db, &issue_weights),
        load_docs(db, &doc_weights),
        load_threads(db, guild_id, &forum_weights),
        load_release_index(db),
    );

    let query_ts = Utc::now();

    let mut all: Vec<Candidate> = Vec::with_capacity(issues.len() + docs.len() + threads.len());
    all.extend(issues);
    all.extend(docs);
    all.extend(threads);

    // ── Build source-specific BM25 statistics ─────────────────────────────────
    // Tokenise each indexed field once so the scoring loop can reuse it.
    let candidate_tokens: Vec<CandidateTokens> = all
        .iter()
        .map(|c| CandidateTokens {
            title: tokenize(&c.title),
            body: tokenize(&c.body),
            comments: tokenize(&c.comments),
        })
        .collect();
    let candidate_buckets: Vec<usize> = all.iter().map(|c| source_bucket(&c.result.kind)).collect();

    let query_tokens = tokenize(query);
    let source_stats = build_source_bm25_stats(&candidate_tokens, &candidate_buckets);

    // ── Pass 1: base_score = BM25 relevance × version_weight × source_weight ─
    for ((c, tokens), bucket) in all
        .iter_mut()
        .zip(candidate_tokens.iter())
        .zip(candidate_buckets.iter())
    {
        let relevance = bm25_score(&query_tokens, tokens, &source_stats[*bucket], weights);
        let candidate_ts = c.result.updated_at.unwrap_or(query_ts);
        let version_wt = release_index.version_weight(query_ts, candidate_ts, weights);

        c.result.score = relevance * version_wt * c.source_weight;
    }

    // ── Filter out low-relevance results ─────────────────────────────────────
    all.retain(|c| c.result.score >= threshold);
    if all.is_empty() {
        return vec![];
    }

    // ── Pass 2: blend in recency — final_score = base_score × recency_mult ───
    //
    // recency ∈ [0, 1]: 0 = oldest candidate, 1 = most recently updated.
    // recency_mult ∈ [recency_base, recency_base + recency_influence].
    // let now_f  = query_ts.timestamp() as f64;
    // let oldest = all
    //     .iter()
    //     .filter_map(|c| c.result.updated_at.map(|dt| dt.timestamp() as f64))
    //     .fold(now_f, f64::min);
    // let time_span = (now_f - oldest).max(1.0); // avoid division by zero

    // for c in &mut all {
    //     let candidate_f  = c.result.updated_at.map_or(oldest, |dt| dt.timestamp() as f64);
    //     let recency      = (candidate_f - oldest) / time_span;
    //     let recency_mult = weights.recency_base + weights.recency_influence * recency;

    //     // println!("Score before recency: {}", c.result.score);
    //     // println!("Recency base: {}", weights.recency_base);
    //     // println!("Recency influence: {}", weights.recency_influence);
    //     // println!("Recency: {}", recency);
    //     // println!("Recency mult: {}", recency_mult);
    //     // println!("Score: {}", c.result.score * recency_mult);

    //     c.result.score *= recency_mult;
    // }

    // ── Sort descending, keep top N ──────────────────────────────────────────
    all.sort_by(|a, b| {
        b.result
            .score
            .partial_cmp(&a.result.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all.truncate(max_results);

    all.into_iter().map(|c| c.result).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(
            parse_version("v4.0.5"),
            Some(Version {
                major: 4,
                minor: 0,
                patch: 5
            })
        );
        assert_eq!(
            parse_version("4.12.0-beta1"),
            Some(Version {
                major: 4,
                minor: 12,
                patch: 0
            })
        );
        assert_eq!(parse_version("invalid"), None);
        assert_eq!(parse_version("v1.2"), None);
    }

    fn test_weights() -> SearchWeights {
        SearchWeights {
            bm25_k1: 1.2,
            bm25_b: 0.75,
            title_weight: 3.0,
            body_weight: 1.0,
            comment_weight: 0.5,
            chunk_docs_by_heading: true,
            version_major_penalty: 12.0,
            version_minor_penalty: 2.0,
            version_patch_penalty: 0.2,
            version_distance_scale: 0.3,
            version_index_distance_scale: 1.5,
        }
    }

    #[test]
    fn test_bm25_title_matches_rank_above_body_only_matches() {
        let query = tokenize("install modpack configuration");
        let candidates = vec![
            CandidateTokens {
                title: tokenize("install modpack configuration"),
                body: tokenize("general reference material"),
                comments: vec![],
            },
            CandidateTokens {
                title: tokenize("general reference"),
                body: tokenize("install modpack configuration"),
                comments: vec![],
            },
        ];
        let stats = build_source_bm25_stats(&candidates, &[1, 1]);
        let weights = test_weights();

        assert!(
            bm25_score(&query, &candidates[0], &stats[1], &weights)
                > bm25_score(&query, &candidates[1], &stats[1], &weights)
        );
    }

    #[test]
    fn test_source_bm25_is_independent_of_other_source_sizes() {
        let query = tokenize("install modpack configuration");
        let docs = vec![CandidateTokens {
            title: tokenize("installation guide"),
            body: tokenize("install modpack configuration"),
            comments: vec![],
        }];
        let doc_only_stats = build_source_bm25_stats(&docs, &[1]);

        let mut mixed_candidates = docs.clone();
        let mut mixed_buckets = vec![1];
        for i in 0..1_000 {
            mixed_candidates.push(CandidateTokens {
                title: tokenize(&format!("install issue {}", i)),
                body: tokenize("modpack configuration failure"),
                comments: vec![],
            });
            mixed_buckets.push(0);
        }
        let mixed_stats = build_source_bm25_stats(&mixed_candidates, &mixed_buckets);
        let weights = test_weights();

        assert_eq!(
            bm25_score(&query, &docs[0], &doc_only_stats[1], &weights),
            bm25_score(&query, &docs[0], &mixed_stats[1], &weights)
        );
    }
}
