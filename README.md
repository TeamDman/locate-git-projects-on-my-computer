# locate-git-projects-on-my-computer

Locate source projects on disk by combining git repository signals with package metadata discovered from files such as `Cargo.toml`.

The current implementation uses `teamy-mft` as the indexed discovery backend, then enriches each discovered project path with git metadata from `git2` and Cargo metadata from `facet-toml`.

## Current Status

The first real implementation slice is in place.

- indexed discovery comes from Teamy MFT search indexes
- `.git` and `Cargo.toml` markers are merged by project directory
- git remotes and git author identities are gathered with `git2`
- Cargo package names, links, and authors are gathered from `Cargo.toml`
- optional `--name`, `--author`, and `--url` filters narrow the emitted results

## Usage

Run the default discovery command:

```powershell
cargo run --
```

This requires Teamy MFT synced index data to be available.

Write logs to disk while keeping human-readable logs on stderr:

```powershell
cargo run -- --log-file .\logs
```

Filter the emitted projects by exact discovered metadata values:

```powershell
cargo run -- --name locate-git-projects-on-my-computer --author "TeamDman" --url https://github.com/TeamDman/locate-git-projects-on-my-computer
```

## Output Shape

The default command emits a pretty-printed JSON array. Each element is intended to represent one discovered on-disk project entry with:

- `path_on_disk`
- `names`
- `outlinks`
- `authors`

## Environment Variables

- `LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_HOME_DIR`: overrides the resolved application home directory
- `LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_CACHE_DIR`: overrides the resolved cache directory
- `TEAMY_MFT_SYNC_DIR`: overrides the Teamy MFT sync directory used for indexed discovery
- `RUST_LOG`: provides a tracing filter when `--log-filter` is not supplied

## Quality Gate

```powershell
./check-all.ps1
```

## Tracy Profiling

```powershell
./run-tracing.ps1
```
