# Yet Another General Support Discord Bot (YAGSDB)

YAGSDB is a Discord forum assistant for communities with an existing knowledge base. It indexes GitHub issues, Markdown documentation from Git repositories, and Discord forum threads, then suggests relevant results when someone opens a new forum post.

The bot is written in Rust and stores its local search index in SQLite.

## What it does

- Syncs issues, comments, and releases from configured GitHub repositories.
- Clones configured Git repositories and indexes `.md` and `.mdx` documentation.
- Imports messages from configured Discord forum threads, including archived threads.
- Searches across all sources using TF-IDF cosine similarity.
- Ranks matches using source weights and proximity to GitHub release versions.
- Optionally posts suggestions automatically when a new forum thread is created.

Background GitHub and Git-documentation syncs run once an hour. Discord forum content is synchronized when the bot becomes ready and is updated from Discord events.

## Requirements

- A current Rust toolchain (the project uses Rust edition 2024).
- Git, for cloning and refreshing documentation repositories.
- A Discord application and bot token.
- A GitHub personal access token if you want GitHub issue and release synchronization.

In the Discord Developer Portal, enable the **Message Content Intent** for the bot. Invite it with access to the configured forum channels; it needs permission to view channels, read message history, and send messages when automatic replies are enabled.

## Quick start

1. Clone the repository and enter it.

```sh
git clone https://github.com/caiostoduto/YAGSDB.git
cd YAGSDB
```

2. Build the project.

```sh
cargo build --release
```

3. Create your local configuration.

```sh
cp config.example.yaml config.yaml
```

4. Edit `config.yaml`, at minimum setting `discord_token`. Configure the data sources you want to search.

5. Run the bot.

```sh
cargo run --release
```

On first run, YAGSDB creates `sqlite.db` in the working directory. Documentation repositories are cloned under `repositories/`.

## Configuration

[`config.example.yaml`](config.example.yaml) documents every available option. The main sections are:

| Section | Purpose |
| --- | --- |
| `discord_token` | Token for the Discord bot. |
| `github_token` | Token used for GitHub API synchronization. Leave the GitHub repository list empty if you do not use this source. |
| `threshold` | Minimum similarity score from `0.0` to `1.0` for a suggestion to be shown. |
| `max_results` | Maximum number of suggestions per new thread. |
| `suggestion_header` | Text displayed before automatic suggestions. |
| `bot_presence` | Discord activity and status. |
| `data_repositories` | GitHub, Git documentation, and Discord forum sources to index. |
| `search_weights` | Controls the effect of release-version distance on ranking. |

Each configured source has a `weight`. Values greater than `1.0` prioritize that source; values below `1.0` de-emphasize it.

### GitHub sources

Add repositories in `owner/name` form under `data_repositories.github_issues`:

```yaml
github_issues:
  - github_repo: "owner/project"
    weight: 1.0
```

Issues and their comments are searchable. Pull requests are synchronized for tracking but are excluded from search results. Releases are used to favor results that are closer to the question's likely project version.

### Documentation sources

`git_docs` accepts any Git clone URL. Add one or more documentation directories and map their local paths to public documentation URLs:

```yaml
git_docs:
  - repository: "https://github.com/owner/project.git"
    weight: 1.5
    docs:
      - path: "./docs"
        url_mapping:
          - from: "./docs"
            to: "https://docs.example.com"
```

Only Markdown and MDX files are indexed. `_meta.json` files are used when present to provide friendlier document titles.

### Discord forums

Configure the guild and forum-channel snowflake IDs under `discord_forums`:

```yaml
discord_forums:
  - guild_id: "123456789012345678"
    channel_id: "123456789012345678"
    weight: 1.0
    reply: true
```

When `reply` is `true`, the bot searches using the new thread's title and opening message, then posts matching documentation, GitHub issues, and existing Discord threads. Set it to `false` to index a forum without sending suggestions there.

## Search behavior

YAGSDB tokenizes the query and indexed content, computes TF-IDF cosine similarity, filters results below `threshold`, and returns the top `max_results` matches. Source weights are applied during ranking. For repositories with releases, matches associated with a nearer release receive a higher score.

Results are labeled by source in Discord as `Docs`, `GitHub Issue`, or `Discord` and include a percentage score. Document and GitHub results link to their configured public URL where available.

## Development

Run the usual Rust checks before submitting changes:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The build script generates a JSON schema at `target/config-schema.json` from the configuration types, which can be used by YAML-aware editors.

## License

YAGSDB is licensed under the [GNU General Public License v3.0](LICENSE).
