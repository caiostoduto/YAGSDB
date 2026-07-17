use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// Discord Bot Token
    pub discord_token: String,
    /// GitHub Personal Access Token
    pub github_token: String,
    /// Minimum BM25 relevance score for a result to be included in search
    /// suggestions. Lower values return more results but with less precision.
    pub threshold: f64,
    /// Maximum number of search results returned per query.
    pub max_results: usize,
    /// Header message prepended to search suggestions posted in forum threads.
    pub suggestion_header: String,
    /// Bot presence / activity shown in the Discord member list.
    pub bot_presence: BotPresence,
    /// Fine-grained tuning knobs for the two-pass search scoring formula.
    pub search_weights: SearchWeights,
    /// Sources of data to index and search over.
    pub data_repositories: DataRepositories,
}

// ── Bot presence ──────────────────────────────────────────────────────────────

/// Activity type shown next to the bot name in the Discord member list.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    Playing,
    Watching,
    Listening,
    Competing,
}

/// Online status for the bot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BotStatus {
    Online,
    Dnd,
    Idle,
    Invisible,
}

/// Bot presence shown in the Discord member list on startup.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BotPresence {
    /// Activity type: playing, watching, listening, or competing.
    pub activity: ActivityKind,
    /// The text displayed next to the activity type (e.g. "for questions").
    pub message: String,
    /// Online status: online, dnd, idle, or invisible.
    pub status: BotStatus,
}

// ── Search scoring weights ────────────────────────────────────────────────────

/// Tuning knobs for BM25 retrieval and score adjustments.
///
/// **Pass 1 — base score** (computed per candidate before filtering):
/// ```text
/// relevance   = field-weighted BM25 score between query and candidate
/// version_wt  = 1 / (1 + semantic_version_distance × version_distance_scale)
/// source_wt   = per-entry weight from data_repositories config
///
/// base_score  = relevance × version_wt × source_wt
/// ```
///
/// **Pass 2 — recency blend** (applied after threshold filtering):
/// ```text
/// recency      = (updated_at − oldest_updated_at) / time_span   ∈ [0, 1]
/// recency_mult = recency_base + recency_influence × recency      ∈ [recency_base, recency_base + recency_influence]
///
/// final_score  = base_score × recency_mult
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchWeights {
    // ── BM25 field scoring ──────────────────────────────────────────────────
    /// Controls how strongly repeated query terms affect a field's score.
    pub bm25_k1: f64,
    /// Controls document-length normalisation. `0.0` disables it; the usual
    pub bm25_b: f64,
    /// Multiplier for matches in titles, including issue, document, and thread
    /// titles. A higher value makes concise title matches rank more strongly.
    pub title_weight: f64,
    /// Multiplier for matches in the main issue, document-section, or thread
    /// body.
    pub body_weight: f64,
    /// Multiplier for matches in GitHub issue comments. Kept lower than body
    /// matches by default because comments are often noisier.
    pub comment_weight: f64,
    /// Split Markdown and MDX files at headings before indexing. This produces
    /// more precise doc matches and URLs with heading anchors.
    pub chunk_docs_by_heading: bool,

    // ── Version distance ─────────────────────────────────────────────────────
    //
    // semantic_distance = major_diff × version_major_penalty
    //                   + minor_diff × version_minor_penalty
    //                   + patch_diff × version_patch_penalty
    //
    // version_weight = 1 / (1 + semantic_distance × version_distance_scale)
    /// Penalty per major-version difference (e.g. v1.x vs v2.x).
    /// Large value: results from a different major version score much lower.
    pub version_major_penalty: f64,
    /// Penalty per minor-version difference (e.g. v1.0 vs v1.2).
    pub version_minor_penalty: f64,
    /// Penalty per patch-version difference (e.g. v1.0.0 vs v1.0.3).
    /// Small value: patch differences have minimal effect on ranking.
    pub version_patch_penalty: f64,
    /// Steepness of the version-distance decay curve.
    /// Higher → stronger preference for same-version results.
    /// Lower  → version proximity matters less.
    pub version_distance_scale: f64,
    /// Decay steepness used when release tags cannot be parsed to SemVer.
    /// The fallback distance is normalised to [0, 1] by index size, so this
    /// scale is comparable across projects with different release cadences.
    pub version_index_distance_scale: f64,
    // // ── Recency blend ────────────────────────────────────────────────────────
    // //
    // // recency      ∈ [0, 1]  (0 = oldest candidate, 1 = most recently updated)
    // // recency_mult = recency_base + recency_influence × recency

    // /// Minimum recency multiplier, applied even to the oldest result.
    // /// Must be in (0, 1]. Keeps stale-but-relevant results from being buried.
    // pub recency_base: f64,
    // /// Additional multiplier awarded to the most recently updated result.
    // /// The effective range is [recency_base, recency_base + recency_influence].
    // pub recency_influence: f64,
}

// ── Data repositories ─────────────────────────────────────────────────────────

/// All data sources the bot indexes and searches over.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DataRepositories {
    /// GitHub repositories to sync issues and releases from.
    /// Issues and releases are fetched via the GitHub REST API.
    pub github_issues: Vec<GithubIssueSource>,
    /// Git repositories to clone and index documentation from.
    /// Any git-accessible URL is supported (not just GitHub).
    pub git_docs: Vec<GitDocsSource>,
    /// Discord forum channels to sync threads and messages from.
    pub discord_forums: Vec<DiscordForum>,
}

// ── GitHub issue source ───────────────────────────────────────────────────────

/// A GitHub repository whose issues and releases are synced via the API.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GithubIssueSource {
    /// GitHub repository in `"owner/name"` format (e.g. `"Skidamek/AutoModpack"`).
    pub github_repo: String,
    /// Score multiplier applied to all search results from this repository.
    /// Use values > 1.0 to surface this repo's results higher, < 1.0 to suppress them.
    pub weight: f64,
}

// ── Git docs source ───────────────────────────────────────────────────────────

/// A git repository to clone locally and index documentation files from.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitDocsSource {
    /// Full git clone URL (e.g. `"https://github.com/Skidamek/AutoModpack.git"`).
    /// Any git-accessible URL works — GitHub, GitLab, self-hosted, etc.
    pub repository: String,
    /// Score multiplier applied to all search results from this repository's docs.
    /// Use values > 1.0 to surface these docs above issues/threads of equal relevance.
    pub weight: f64,
    /// Documentation paths within the cloned repository to index.
    pub docs: Vec<DocSet>,
}

/// A single documentation directory within a cloned git repository.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocSet {
    /// Path to the docs directory relative to the repository root (e.g. `"./docs"`).
    pub path: String,
    /// URL mappings that translate local file paths to public web URLs.
    pub url_mapping: Vec<UrlMapping>,
}

/// Translates a local file path prefix to a public URL prefix.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UrlMapping {
    /// Local path prefix to match (e.g. `"./docs"`).
    pub from: String,
    /// Public URL prefix to replace it with (e.g. `"https://example.com/docs"`).
    pub to: String,
}

// ── Discord forum source ──────────────────────────────────────────────────────

/// A Discord forum channel whose threads and messages are synced and searched.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscordForum {
    /// Discord Guild (server) ID as a string snowflake.
    pub guild_id: String,
    /// Discord Channel ID of the forum channel as a string snowflake.
    pub channel_id: String,
    /// Score multiplier applied to all search results from this forum.
    pub weight: f64,
    /// When `true`, the bot posts search suggestions in reply to new threads.
    pub reply: bool,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

impl Config {
    #[allow(dead_code)]
    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::verify()?;
        let content = std::fs::read_to_string("config.yaml")?;
        let config = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Verify that the config file exists and is valid
    pub fn verify() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !std::path::Path::new("config.yaml").exists() {
            if std::path::Path::new("config.example.yaml").exists() {
                std::fs::copy("config.example.yaml", "config.yaml")?;
                return Err(
                    "Warning: config.yaml was not found. A new one has been created from config.example.yaml. Please fill in the required fields and run the bot again.".into()
                );
            } else {
                return Err(
                    "Error: config.yaml not found and config.example.yaml is missing.".into(),
                );
            }
        }

        Ok(())
    }
}
