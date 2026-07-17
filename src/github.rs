use crate::config::{Config, GithubIssueSource};
use reqwest::Client;
use serde::Deserialize;
use sqlx::{Row, SqlitePool};
use std::time::Duration;

// ── GitHub API models ────────────────────────────────────────────────────────

/// Present when the item is a pull request.
#[derive(Deserialize, Debug)]
struct GhPullRequest {}

#[derive(Deserialize, Debug)]
struct GhIssue {
    id: u64,
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    updated_at: String,
    comments_url: String,
    /// Only present on pull requests.
    pull_request: Option<GhPullRequest>,
}

#[derive(Deserialize, Debug)]
struct GhComment {
    id: u64,
    body: Option<String>,
    updated_at: String,
}

#[derive(Deserialize, Debug)]
struct GhRelease {
    id: u64,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
}

// ── Per-repo sync metrics ────────────────────────────────────────────────────

struct SyncMetrics {
    issues: usize,
    prs: usize,
    comments: usize,
    releases: usize,
}

impl SyncMetrics {
    fn new() -> Self {
        Self {
            issues: 0,
            prs: 0,
            comments: 0,
            releases: 0,
        }
    }

    fn add(&mut self, other: &SyncMetrics) {
        self.issues += other.issues;
        self.prs += other.prs;
        self.comments += other.comments;
        self.releases += other.releases;
    }

    fn total_items(&self) -> usize {
        self.issues + self.prs + self.releases
    }
}

// ── HTTP helpers ─────────────────────────────────────────────────────────────

fn build_http_client() -> Client {
    Client::builder()
        .user_agent("DiscordWikiBot")
        .build()
        .unwrap()
}

/// Fetch all pages of a paginated GitHub API endpoint, returning all items.
async fn fetch_all_pages<T: for<'de> Deserialize<'de>>(
    client: &Client,
    base_url: &str,
    token: &str,
) -> Vec<T> {
    let mut all_items: Vec<T> = Vec::new();
    let mut page = 1u32;
    let per_page = 100;

    loop {
        let url = paginated_url(base_url, per_page, page);
        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;

        match resp {
            Ok(r) => match r.json::<Vec<T>>().await {
                Ok(items) => {
                    let count = items.len();
                    all_items.extend(items);

                    if count < per_page {
                        break; // Last page reached
                    }
                    page += 1;
                }
                Err(e) => {
                    eprintln!("[github.rs] Failed to deserialize page {}: {}", page, e);
                    break;
                }
            },
            Err(e) => {
                eprintln!("[github.rs] Request failed on page {}: {}", page, e);
                break;
            }
        }
    }

    all_items
}

/// Build a URL with `per_page` and `page` query parameters appended.
fn paginated_url(base_url: &str, per_page: usize, page: u32) -> String {
    let sep = if base_url.contains('?') { '&' } else { '?' };
    format!(
        "{}{sep}per_page={}&page={}",
        base_url,
        per_page,
        page,
        sep = sep
    )
}

// ── Timestamp helpers ────────────────────────────────────────────────────────

/// Advance an ISO 8601 timestamp by one second so the GitHub `since` filter
/// is effectively exclusive (GitHub's `since` is inclusive).
fn advance_timestamp_by_one_sec(ts: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        let advanced = dt + chrono::Duration::seconds(1);
        return advanced.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    }
    ts.to_string()
}

// ── Database helpers ─────────────────────────────────────────────────────────

/// Returns the most recent `updated_at` timestamp stored for a given repo,
/// or `None` if no issues have been synced yet.
async fn get_last_updated_at(db: &SqlitePool, repo_name: &str) -> Option<String> {
    let row = sqlx::query(
        "SELECT updated_at FROM gh_issues WHERE repo = ? ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(repo_name)
    .fetch_optional(db)
    .await
    .unwrap_or(None)?;

    row.try_get::<String, _>("updated_at").ok()
}

/// Upsert a single issue (or PR) into the database.
async fn upsert_issue(db: &SqlitePool, repo_name: &str, item: &GhIssue) {
    let item_id_str = item.id.to_string();
    let is_pr = item.pull_request.is_some();

    let _ = sqlx::query(
        "INSERT OR REPLACE INTO gh_issues \
            (id, repo, number, title, body, state, is_pr, updated_at) \
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&item_id_str)
    .bind(repo_name)
    .bind(item.number as i64)
    .bind(&item.title)
    .bind(item.body.as_deref().unwrap_or(""))
    .bind(&item.state)
    .bind(is_pr)
    .bind(&item.updated_at)
    .execute(db)
    .await;
}

/// Upsert a single comment into the database.
async fn upsert_comment(db: &SqlitePool, issue_id: &str, comment: &GhComment) {
    let comment_id_str = comment.id.to_string();

    let _ = sqlx::query(
        "INSERT OR REPLACE INTO gh_comments \
            (id, issue_id, body, updated_at) \
            VALUES (?, ?, ?, ?)",
    )
    .bind(&comment_id_str)
    .bind(issue_id)
    .bind(comment.body.as_deref().unwrap_or(""))
    .bind(&comment.updated_at)
    .execute(db)
    .await;
}

/// Upsert a single release into the database. Returns `true` if successful.
async fn upsert_release(db: &SqlitePool, repo_name: &str, release: &GhRelease) -> bool {
    let release_id_str = release.id.to_string();

    sqlx::query(
        "INSERT OR IGNORE INTO gh_releases \
            (id, repo, tag_name, name, body, published_at) \
            VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&release_id_str)
    .bind(repo_name)
    .bind(&release.tag_name)
    .bind(release.name.as_deref().unwrap_or(""))
    .bind(release.body.as_deref().unwrap_or(""))
    .bind(release.published_at.as_deref().unwrap_or(""))
    .execute(db)
    .await
    .map(|r| r.rows_affected() > 0)
    .unwrap_or(false)
}

// ── Per-repo sync logic ──────────────────────────────────────────────────────

/// Build the issues list URL, appending a `since` parameter when we have a
/// previously-stored timestamp so we only fetch newly-updated items.
async fn build_issues_url(db: &SqlitePool, repo_name: &str) -> String {
    let base = format!(
        "https://api.github.com/repos/{}/issues?state=all&sort=updated&direction=asc",
        repo_name
    );

    match get_last_updated_at(db, repo_name).await {
        Some(last_updated) => {
            let exclusive_since = advance_timestamp_by_one_sec(&last_updated);
            format!("{}&since={}", base, exclusive_since)
        }
        None => base,
    }
}

/// Fetch and upsert all comments for a single issue. Returns the number of
/// comments synced.
async fn sync_issue_comments(
    db: &SqlitePool,
    client: &Client,
    token: &str,
    issue: &GhIssue,
) -> usize {
    let comments = fetch_all_pages::<GhComment>(client, &issue.comments_url, token).await;
    let issue_id_str = issue.id.to_string();
    let count = comments.len();

    for comment in &comments {
        upsert_comment(db, &issue_id_str, comment).await;
    }

    count
}

/// Fetch and upsert all releases for a repo. Returns the number of releases
/// successfully written to the database.
async fn sync_releases(db: &SqlitePool, client: &Client, token: &str, repo_name: &str) -> usize {
    let url = format!("https://api.github.com/repos/{}/releases", repo_name);
    let releases = fetch_all_pages::<GhRelease>(client, &url, token).await;

    let mut count = 0;
    for release in &releases {
        if upsert_release(db, repo_name, release).await {
            count += 1;
        }
    }
    count
}

/// Run a full sync cycle for a single repository and return the metrics.
async fn sync_repo(
    db: &SqlitePool,
    client: &Client,
    token: &str,
    repo: &GithubIssueSource,
) -> SyncMetrics {
    let repo_name = &repo.github_repo;
    let mut metrics = SyncMetrics::new();

    // ── Issues & PRs ─────────────────────────────────────────────────────────
    let issues_url = build_issues_url(db, repo_name).await;
    let items = fetch_all_pages::<GhIssue>(client, &issues_url, token).await;

    for item in &items {
        let is_pr = item.pull_request.is_some();

        upsert_issue(db, repo_name, item).await;
        metrics.comments += sync_issue_comments(db, client, token, item).await;

        if is_pr {
            metrics.prs += 1;
        } else {
            metrics.issues += 1;
        }
    }

    // ── Releases ─────────────────────────────────────────────────────────────
    metrics.releases = sync_releases(db, client, token, repo_name).await;

    if metrics.total_items() > 0 {
        println!(
            "[github.rs] {}: +{} issues, +{} PRs, +{} comments, +{} releases",
            repo_name, metrics.issues, metrics.prs, metrics.comments, metrics.releases
        );
    }

    metrics
}

// ── Sync cycle & job entry point ─────────────────────────────────────────────

/// Run one full sync cycle across all configured repositories.
async fn run_sync_cycle(
    db: &SqlitePool,
    client: &Client,
    token: &str,
    repos: &[GithubIssueSource],
) {
    let mut totals = SyncMetrics::new();

    for repo in repos {
        let metrics = sync_repo(db, client, token, repo).await;
        totals.add(&metrics);
    }

    if totals.total_items() > 0 {
        println!(
            "[github.rs] Sync complete — +{} issues, +{} PRs, +{} comments, +{} releases",
            totals.issues, totals.prs, totals.comments, totals.releases
        );
    } else {
        println!("[github.rs] Sync complete — nothing new");
    }
}

pub async fn start_sync_job(db: SqlitePool, config: Config) {
    let client = build_http_client();
    let token = config.github_token.clone();

    if token.is_empty() {
        eprintln!("[github.rs] No token provided, skipping sync job");
        return;
    }

    let gh_repos = &config.data_repositories.github_issues;

    if gh_repos.is_empty() {
        eprintln!("[github.rs] No github repositories defined, skipping sync job");
        return;
    }

    loop {
        run_sync_cycle(&db, &client, &token, gh_repos).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
