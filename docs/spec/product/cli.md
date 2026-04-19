# CLI

This specification covers the current user-facing command line behavior exposed by `locate-git-projects-on-my-computer`.

## Command Surface

cli[command.surface.default-discovery]
The CLI must successfully run with no positional arguments or subcommands.

cli[command.surface.named-filters]
The CLI must support repeated named filters for project names, authors, and URLs.

cli[command.surface.discovery-tuning-flags]
The CLI should accept optional named tuning flags for enrichment parallelism and git author scanning.

cli[command.surface.named-filters-or-semantics]
When one or more name, author, or URL filters are supplied, a project must be included if it contains at least one requested value in any of those metadata collections.

cli[command.surface.name-filter-dash-underscore-normalization]
Name filter matching must treat `-` and `_` as equivalent so that values such as `rc-zip` and `rc_zip` match one another.

cli[command.surface.author-filter-case-insensitive-substring]
Author filter matching must treat the provided value as a case-insensitive substring of a discovered author string rather than requiring full-string equality.

cli[command.surface.activity-filter]
When `--activity` is provided, the CLI must keep only git repositories whose branch tips reach at least one commit newer than now minus the provided humantime duration.

cli[command.surface.default-json]
When run with no positional arguments or subcommands, the CLI must write a pretty-printed JSON array to stdout.

cli[command.surface.default-noise-free]
The default JSON output must be emitted on stdout so that logs can remain on stderr.

## Parser Model

cli[parser.args-consistent]
The structured CLI model must serialize to command line arguments consistently for parse-safe values.

cli[parser.roundtrip]
The structured CLI model must roundtrip through argument serialization and parsing for parse-safe values.

## Discovery Inputs

cli[discovery.git-directories]
The discovery pipeline must search for directories named `.git` and ignore suffix matches that are not actually directories named `.git`.

cli[discovery.cargo-toml]
The discovery pipeline must search for `Cargo.toml` files so source trees without a `.git` directory can still produce project entries.

cli[discovery.requires-teamy-mft-index]
The initial implementation may depend on Teamy MFT's synced search index data being available for indexed discovery.

cli[discovery.teamy-mft-sync-dir-env]
The implementation must honor `TEAMY_MFT_SYNC_DIR` when Teamy MFT uses that environment variable to select the synced index location.

cli[discovery.query-pattern.git]
The initial implementation must use Teamy MFT indexed lookup for `.git$` and then filter results to exact directories named `.git`.

cli[discovery.query-pattern.cargo-toml]
The initial implementation must use Teamy MFT indexed lookup for `Cargo.toml$` and then filter results to exact files named `Cargo.toml`.

cli[discovery.parallel-candidate-queries]
The indexed candidate lookups for `.git` directories and `Cargo.toml` files should be able to run concurrently before metadata enrichment begins.

cli[discovery.serial-query-phase]
The initial MFT-backed discovery phase should complete before the metadata-enrichment phase starts so the implementation stays simple.

cli[discovery.parallel-enrichment]
After discovery identifies candidate paths, metadata enrichment should be able to run concurrently.

cli[discovery.best-effort-enrichment]
Metadata enrichment failures for an individual discovered project should not abort the entire discovery run; the project entry should still be emitted with whatever metadata was gathered successfully.

## Metadata Enrichment

cli[enrichment.gix-repository-metadata]
For discovered git repositories, the implementation must use the `gix` crate rather than shelling out to the `git` command in order to gather repository metadata.

cli[enrichment.git-remotes]
For discovered git repositories, the implementation must gather outlinks from configured git remotes.

cli[enrichment.git-authors]
For discovered git repositories, the implementation must gather author identities from reachable local commit history.

cli[enrichment.git-author-scan-minimum]
For discovered git repositories, author scanning must inspect at least the configured minimum number of reachable commits before time budgeting can stop the walk.

cli[enrichment.git-author-scan-budget]
After the configured minimum scan depth is satisfied, git author scanning should stop once the per-repository time budget is exhausted.

cli[enrichment.cargo-manifest]
For discovered Cargo projects, the implementation must parse `Cargo.toml` with `facet-toml` and gather package authors plus link metadata such as repository, homepage, and documentation when present.

cli[enrichment.project-names]
The implementation must gather project names from the containing directory name and from `Cargo.toml` package names when present.

## Output Shape

cli[output.entry.path-on-disk]
Each JSON array entry must contain exactly one `path_on_disk` field identifying the discovered location on disk.

cli[output.entry.names]
Each JSON array entry must contain a `names` field with zero or more project names gathered from directory names or package metadata.

cli[output.entry.project-directory]
For `.git` and `Cargo.toml` discoveries, `path_on_disk` must identify the containing project directory rather than the marker file or directory itself.

cli[output.entry.outlinks]
Each JSON array entry must contain an `outlinks` field with zero or more URI strings gathered from git remotes or package metadata.

cli[output.entry.authors]
Each JSON array entry must contain an `authors` field with zero or more author strings gathered from git history or package metadata.

cli[output.entry.last-activity-on]
Each JSON array entry must contain a `last_activity_on` field. When branch activity metadata is available, it must be an RFC 3339 timestamp for the newest discovered branch-tip-reachable commit; otherwise it must be `null`.

cli[output.entry.last-activity-ago]
Each JSON array entry must contain a `last_activity_ago` field. When branch activity metadata is available, it must describe the same newest discovered commit relative to when the command ran; otherwise it must be `null`.

cli[output.entry.authors.git-style]
When both a display name and email are available for an author, the author string should use git-style formatting such as `Name <email@example.com>`.

cli[output.entry.merge-by-path]
When the same project directory is discovered from multiple marker types, the implementation must merge the metadata into a single output entry for that path.

cli[output.entry.sorted-deduped-metadata]
Each output entry should present names, authors, and outlinks in a deterministic deduplicated order.

cli[output.serialization.facet-json]
The JSON output must be serialized with `facet-json` rather than `serde_json`.

## Path Resolution

cli[path.app-home.env-overrides-platform]
If `LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_HOME_DIR` is set to a non-empty value, it must take precedence over the platform-derived application home directory.

cli[path.cache.env-overrides-platform]
If `LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_CACHE_DIR` is set to a non-empty value, it must take precedence over the platform-derived cache directory.