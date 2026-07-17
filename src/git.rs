use crate::config::{Config, DocSet, GitDocsSource};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

// ── Path helpers ─────────────────────────────────────────────────────────────

/// Derive a stable local directory for a git repository from its clone URL.
/// Strips the scheme and replaces path separators so the result is a valid
/// single-level directory name (e.g. `"github.com_Skidamek_AutoModpack"`).
fn repo_dir_for_url(url: &str) -> PathBuf {
    let sanitised = url
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("git@")
        .replace([':', '/'], "_");
    PathBuf::from("repositories").join(sanitised)
}

/// Normalise a config path string: strip leading "./" and trailing "/".
fn norm(p: &str) -> &str {
    p.trim_start_matches("./").trim_end_matches('/')
}

// ── URL computation ──────────────────────────────────────────────────────────

/// Given a file path relative to the repo root (e.g. "docs/faq.mdx") and the
/// list of url_mappings, compute the public URL for that file.
///
/// Mapping logic:
///   from = "docs"  →  strip "docs/" prefix, strip extension, append to `to`
///   e.g. "docs/configuration/server-config.mdx"
///        → "configuration/server-config"
///        → "{to}/configuration/server-config"
fn compute_url(rel_path: &str, mappings: &[crate::config::UrlMapping]) -> Option<String> {
    for mapping in mappings {
        if let Some(url) = try_map_url(rel_path, mapping) {
            return Some(url);
        }
    }
    None
}

fn try_map_url(rel_path: &str, mapping: &crate::config::UrlMapping) -> Option<String> {
    let from = norm(&mapping.from);
    let to = mapping.to.trim_end_matches('/');

    // Require that rel_path equals `from` exactly or starts with `from/`.
    let rest = if rel_path == from {
        ""
    } else {
        rel_path.strip_prefix(&format!("{}/", from))?
    };

    let rest_no_ext = strip_extension(rest);

    Some(if rest_no_ext.is_empty() {
        to.to_string()
    } else {
        format!("{}/{}", to, rest_no_ext)
    })
}

/// Strip the file extension from a path segment (e.g. "page.mdx" → "page").
fn strip_extension(path: &str) -> &str {
    match path.rfind('.') {
        Some(pos) => &path[..pos],
        None => path,
    }
}

// ── Filesystem helpers ───────────────────────────────────────────────────────

/// Recursively collect all .md / .mdx files, skipping hidden directories.
fn collect_docs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip hidden dirs/files (e.g. .translated)
        if name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            out.extend(collect_docs(&path));
        } else if is_doc_file(&path) {
            out.push(path);
        }
    }
    out
}

fn is_doc_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "md" || ext == "mdx")
}

/// Read a file's last-modified time as an RFC-3339 string, falling back to now.
fn file_mtime_rfc3339(path: &Path) -> String {
    path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

// ── _meta.json support ───────────────────────────────────────────────────────

/// A single entry in a `_meta.json` file.
#[derive(serde::Deserialize)]
struct MetaEntry {
    name: Option<String>,
}

/// Load all `_meta.json` files under `docs_dir` and build a mapping from
/// file name (e.g. "quick-start.mdx") or directory name to human-friendly title.
fn load_meta_titles(docs_dir: &Path) -> HashMap<PathBuf, String> {
    let mut titles = HashMap::new();
    load_meta_titles_recursive(docs_dir, docs_dir, &mut titles);
    titles
}

fn load_meta_titles_recursive(
    current_dir: &Path,
    docs_root: &Path,
    titles: &mut HashMap<PathBuf, String>,
) {
    let meta_path = current_dir.join("_meta.json");
    if meta_path.is_file()
        && let Ok(content) = std::fs::read_to_string(&meta_path)
        && let Ok(meta) = serde_json::from_str::<HashMap<String, MetaEntry>>(&content)
    {
        for (key, entry) in meta {
            if let Some(name) = entry.name {
                // Build the full path relative to docs_root
                let rel = current_dir.join(&key);
                // Store relative to docs_root
                if let Ok(stripped) = rel.strip_prefix(docs_root) {
                    titles.insert(stripped.to_path_buf(), name);
                }
            }
        }
    }

    // Recurse into subdirectories
    let Ok(entries) = std::fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name_str = entry.file_name().to_string_lossy().to_string();
        if name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            load_meta_titles_recursive(&path, docs_root, titles);
        }
    }
}

/// Look up the title for a doc file from the _meta.json mapping.
/// `file_rel_to_docs` is the file path relative to the docs root,
/// e.g. "configuration/server-config.mdx".
fn lookup_meta_title(titles: &HashMap<PathBuf, String>, file_rel_to_docs: &str) -> Option<String> {
    let path = Path::new(file_rel_to_docs);
    // Try the exact path first (e.g. "quick-start.mdx")
    if let Some(title) = titles.get(path) {
        return Some(title.clone());
    }
    // Also try just the filename in case the key is just the filename
    if let Some(name) = path.file_name() {
        let name_path = Path::new(name);
        if let Some(title) = titles.get(name_path) {
            return Some(title.clone());
        }
    }
    None
}

// ── Git operations ───────────────────────────────────────────────────────────

/// Run a git command in `dir` with all output suppressed.
/// Returns `true` if the command exits successfully.
async fn run_git(args: &[&str], dir: &Path) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Returns the current HEAD commit hash for the repo at `dir`, or `None` on error.
async fn head_commit(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Clone a repository (shallow) into `dir` from a full git URL.
async fn clone_repo(clone_url: &str, dir: &Path) -> RepoSyncResult {
    let parent = dir.parent().unwrap();
    let repo_leaf = dir.file_name().unwrap().to_str().unwrap();

    if let Err(e) = std::fs::create_dir_all(parent) {
        eprintln!("[git.rs] Could not create {}: {}", parent.display(), e);
        return RepoSyncResult::Failed;
    }

    let ok = run_git(&["clone", "--depth=1", clone_url, repo_leaf], parent).await;
    if ok {
        println!("[git.rs] Cloned {}", clone_url);
        RepoSyncResult::Changed
    } else {
        eprintln!("[git.rs] Clone failed for {}", clone_url);
        RepoSyncResult::Failed
    }
}

/// The outcome of an `update_repo` or `clone_repo` call.
enum RepoSyncResult {
    /// Repo was cloned or updated with new commits.
    Changed,
    /// Repo already existed and was already up-to-date.
    Unchanged,
    /// The git operation failed.
    Failed,
}

/// Fetch latest changes and hard-reset an existing local clone.
/// Returns `Changed` only when new commits were actually pulled in.
async fn update_repo(clone_url: &str, dir: &Path) -> RepoSyncResult {
    let before = head_commit(dir).await;

    if !run_git(&["fetch", "--depth=1", "origin"], dir).await {
        eprintln!("[git.rs] fetch failed for {}", clone_url);
        return RepoSyncResult::Failed;
    }
    if !run_git(&["reset", "--hard", "origin/HEAD"], dir).await {
        eprintln!("[git.rs] reset failed for {}", clone_url);
        return RepoSyncResult::Failed;
    }

    let after = head_commit(dir).await;

    if before != after {
        println!("[git.rs] Updated {}", clone_url);
        RepoSyncResult::Changed
    } else {
        RepoSyncResult::Unchanged
    }
}

// ── Doc indexing ─────────────────────────────────────────────────────────────

/// Index a single documentation file into the `docs` DB table.
async fn index_doc_file(
    repo_name: &str,
    file_path: &Path,
    repo_dir: &Path,
    url_mapping: &[crate::config::UrlMapping],
    meta_title: Option<&str>,
    db: &SqlitePool,
) {
    let rel_path = match file_path.strip_prefix(repo_dir) {
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(_) => return,
    };

    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[git.rs] Read error {}: {}", file_path.display(), e);
            return;
        }
    };

    let updated_at = file_mtime_rfc3339(file_path);
    let url = compute_url(&rel_path, url_mapping);
    let id = format!("{}:{}", repo_name, rel_path);

    let _ = sqlx::query(
        "INSERT OR REPLACE INTO docs (id, repo, file_path, url, title, content, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(repo_name)
    .bind(&rel_path)
    .bind(&url)
    .bind(meta_title)
    .bind(&content)
    .bind(&updated_at)
    .execute(db)
    .await;
}

/// Index all configured docs paths for a source into the `docs` DB table.
async fn index_docs(repo_name: &str, doc_sets: &[DocSet], repo_dir: &Path, db: &SqlitePool) {
    if doc_sets.is_empty() {
        return;
    }

    let mut count = 0usize;
    for doc_set in doc_sets {
        let docs_dir = repo_dir.join(norm(&doc_set.path));
        if !docs_dir.exists() {
            eprintln!("[git.rs] Docs path not found: {}", docs_dir.display());
            continue;
        }

        // Load _meta.json titles for this docs directory
        let meta_titles = load_meta_titles(&docs_dir);

        let url_mapping = doc_set.url_mapping.as_slice();
        for file_path in collect_docs(&docs_dir) {
            // Compute the file's path relative to the docs dir for _meta.json lookup
            let meta_title = file_path.strip_prefix(&docs_dir).ok().and_then(|rel| {
                let rel_str = rel.to_string_lossy();
                lookup_meta_title(&meta_titles, &rel_str)
            });

            index_doc_file(
                repo_name,
                &file_path,
                repo_dir,
                url_mapping,
                meta_title.as_deref(),
                db,
            )
            .await;
            count += 1;
        }
    }

    if count > 0 {
        println!("[git.rs] {}: indexed {} doc files", repo_name, count);
    }
}

// ── DB helpers ───────────────────────────────────────────────────────────────

/// Returns the number of doc rows stored for a given repo.
async fn doc_count_for_repo(db: &SqlitePool, repo_name: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM docs WHERE repo = ?")
        .bind(repo_name)
        .fetch_one(db)
        .await
        .unwrap_or(0)
}

// ── Repo sync ────────────────────────────────────────────────────────────────

async fn sync_source(source: &GitDocsSource, db: &SqlitePool) {
    let clone_url = &source.repository;
    let dir = repo_dir_for_url(clone_url);
    // Use a short human-readable label for logs (strip scheme + .git suffix).
    let repo_name = clone_url
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    let result = if dir.exists() {
        update_repo(clone_url, &dir).await
    } else {
        clone_repo(clone_url, &dir).await
    };

    // Re-index docs if git changed, or if DB has no docs for this source
    // (e.g. DB was deleted while repo was already cloned).
    let needs_index = match result {
        RepoSyncResult::Changed => true,
        RepoSyncResult::Failed => false,
        RepoSyncResult::Unchanged => doc_count_for_repo(db, repo_name).await == 0,
    };

    if needs_index {
        index_docs(repo_name, &source.docs, &dir, db).await;
    }
}

fn git_doc_sources(config: &Config) -> &[GitDocsSource] {
    &config.data_repositories.git_docs
}

// ── Public API ───────────────────────────────────────────────────────────────

pub async fn start_sync_job(db: SqlitePool, config: Config) {
    loop {
        for source in git_doc_sources(&config) {
            sync_source(source, &db).await;
        }
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
