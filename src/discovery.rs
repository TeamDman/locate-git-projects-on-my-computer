use eyre::Context;
use facet::Facet;
use gix::Repository;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tracing::debug;
use tracing::info_span;
use tracing::warn;

const GIT_QUERY_PATTERN: &str = ".git$";
const CARGO_TOML_QUERY_PATTERN: &str = "Cargo.toml$";
const DEFAULT_ENRICHMENT_MAX_IN_FLIGHT: usize = 64;
const DEFAULT_AUTHOR_MIN_COMMITS: usize = 1;
const DEFAULT_AUTHOR_SCAN_CHUNK_SIZE: usize = 64;
const DEFAULT_AUTHOR_SCAN_BUDGET: Duration = Duration::ZERO;

fn default_enrichment_max_in_flight() -> usize {
    DEFAULT_ENRICHMENT_MAX_IN_FLIGHT
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryConfig {
    pub max_in_flight: Option<usize>,
    pub author_min_commits: usize,
    pub author_scan_budget: Duration,
    pub author_scan_chunk_size: usize,
    pub activity_cutoff: Option<SystemTime>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            max_in_flight: Some(default_enrichment_max_in_flight()),
            author_min_commits: DEFAULT_AUTHOR_MIN_COMMITS,
            author_scan_budget: DEFAULT_AUTHOR_SCAN_BUDGET,
            author_scan_chunk_size: DEFAULT_AUTHOR_SCAN_CHUNK_SIZE,
            activity_cutoff: None,
        }
    }
}

impl DiscoveryConfig {
    #[must_use]
    pub fn with_overrides(
        max_in_flight: Option<usize>,
        author_min_commits: Option<usize>,
        author_scan_budget_ms: Option<u64>,
        author_scan_chunk_size: Option<usize>,
        activity_cutoff: Option<SystemTime>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            max_in_flight: max_in_flight.or(defaults.max_in_flight),
            author_min_commits: author_min_commits.unwrap_or(DEFAULT_AUTHOR_MIN_COMMITS),
            author_scan_budget: author_scan_budget_ms
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_AUTHOR_SCAN_BUDGET),
            author_scan_chunk_size: author_scan_chunk_size
                .unwrap_or(DEFAULT_AUTHOR_SCAN_CHUNK_SIZE),
            activity_cutoff,
        }
        .sanitized()
    }

    #[must_use]
    fn sanitized(self) -> Self {
        Self {
            max_in_flight: self.max_in_flight.filter(|value| *value > 0),
            author_min_commits: self.author_min_commits,
            author_scan_budget: self.author_scan_budget,
            author_scan_chunk_size: self.author_scan_chunk_size.max(1),
            activity_cutoff: self.activity_cutoff,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProjectRecord {
    pub project: DiscoveredProject,
    pub newest_branch_activity_at: Option<SystemTime>,
}

impl DiscoveredProjectRecord {
    #[must_use]
    pub fn into_project(self) -> DiscoveredProject {
        self.into_project_at(SystemTime::now())
    }

    #[must_use]
    pub fn into_project_at(self, now: SystemTime) -> DiscoveredProject {
        let Self {
            mut project,
            newest_branch_activity_at,
        } = self;
        let (last_activity_on, last_activity_ago) = activity_output_fields(newest_branch_activity_at, now);
        project.last_activity_on = last_activity_on;
        project.last_activity_ago = last_activity_ago;
        project
    }
}

/// A single source project found on disk.
#[derive(Facet, Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveredProject {
    /// The on-disk path for this specific discovered project entry.
    pub path_on_disk: String,
    /// Project names gathered from directory names or package metadata.
    pub names: Vec<String>,
    /// Source repository or file URIs associated with this entry.
    pub outlinks: Vec<String>,
    /// Authors gathered from repository history or package metadata.
    pub authors: Vec<String>,
    /// The newest discovered branch activity as an RFC 3339 timestamp.
    pub last_activity_on: Option<String>,
    /// The newest discovered branch activity relative to when the command ran.
    pub last_activity_ago: Option<String>,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq, Default)]
struct CargoManifest {
    package: Option<CargoPackage>,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq, Default)]
struct CargoPackage {
    name: Option<String>,
    repository: Option<CargoStringField>,
    homepage: Option<CargoStringField>,
    documentation: Option<CargoStringField>,
    authors: Option<CargoStringListField>,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
struct WorkspaceField {
    workspace: bool,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
#[repr(u8)]
#[facet(untagged)]
enum CargoStringField {
    Value(String),
    Workspace(WorkspaceField),
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
#[repr(u8)]
#[facet(untagged)]
enum CargoStringListField {
    Value(Vec<String>),
    Workspace(WorkspaceField),
}

#[derive(Debug, Clone)]
enum DiscoverySeed {
    GitDir(PathBuf),
    CargoToml(PathBuf),
}

#[derive(Debug, Clone, Default)]
struct PartialProject {
    path_on_disk: String,
    names: BTreeSet<String>,
    outlinks: BTreeSet<String>,
    authors: BTreeSet<String>,
    newest_branch_activity_at: Option<SystemTime>,
}

/// Discover source projects on disk.
///
/// The implementation uses Teamy MFT's indexed query API to discover `.git`
/// directories and `Cargo.toml` files, then enriches each discovered project
/// path with best-effort metadata from git history and Cargo package metadata.
///
/// # Errors
///
/// This function will return an error if the indexed discovery query fails,
/// such as when Teamy MFT has no configured sync directory or no readable index
/// data is available.
pub async fn discover_projects() -> eyre::Result<Vec<DiscoveredProject>> {
    discover_projects_with_config(DiscoveryConfig::default()).await
}

pub async fn discover_project_records_with_config(
    config: DiscoveryConfig,
) -> eyre::Result<Vec<DiscoveredProjectRecord>> {
    let config = config.sanitized();
    let author_scan_budget_ms = u64::try_from(config.author_scan_budget.as_millis())
        .unwrap_or(u64::MAX);
    let _span = info_span!(
        "discover_projects",
        max_in_flight = config.max_in_flight.unwrap_or(0),
        author_min_commits = config.author_min_commits,
        author_scan_budget_ms,
        author_scan_chunk_size = config.author_scan_chunk_size,
        activity_filter = config.activity_cutoff.is_some(),
    )
    .entered();

    let seeds = {
        let _span = info_span!("discover_candidate_paths").entered();
        discover_candidate_paths().await?
    };
    let mut partials = Vec::with_capacity(seeds.len());
    let mut join_set = JoinSet::new();

    {
        let _span = info_span!(
            "dispatch_enrichment_tasks",
            seeds = seeds.len(),
            max_in_flight = config.max_in_flight.unwrap_or(0),
        )
        .entered();

        for seed in seeds {
            while config
                .max_in_flight
                .is_some_and(|max_in_flight| join_set.len() >= max_in_flight)
            {
                partials.push(join_next_partial(&mut join_set).await?);
            }

            join_set.spawn_blocking(move || enrich_seed(seed, config));
        }
    }

    {
        let _span = info_span!("collect_enrichment_results").entered();
        while !join_set.is_empty() {
            partials.push(join_next_partial(&mut join_set).await?);
        }
    }

    Ok(info_span!("merge_discovered_projects", partials = partials.len())
        .in_scope(|| merge_partials(partials, config.activity_cutoff)))
}

// cli[discovery.parallel-candidate-queries]
// cli[discovery.parallel-enrichment]
// cli[discovery.best-effort-enrichment]
// tool[profiling.discovery-phases-spanned]
// tool[profiling.discovery-bounded-fields]
pub async fn discover_projects_with_config(
    config: DiscoveryConfig,
) -> eyre::Result<Vec<DiscoveredProject>> {
    let now = SystemTime::now();
    Ok(discover_project_records_with_config(config)
        .await?
        .into_iter()
        .map(|record| record.into_project_at(now))
        .collect())
}

async fn join_next_partial(
    join_set: &mut JoinSet<eyre::Result<PartialProject>>,
) -> eyre::Result<PartialProject> {
    join_set
        .join_next()
        .await
        .ok_or_else(|| eyre::eyre!("enrichment task set unexpectedly empty"))?
        .wrap_err("discovery enrichment task failed")?
}

// cli[discovery.query-pattern.git]
// cli[discovery.query-pattern.cargo-toml]
async fn discover_candidate_paths() -> eyre::Result<Vec<DiscoverySeed>> {
    let git_dirs_task = tokio::task::spawn_blocking(|| {
        let _span = info_span!("query_git_directories").entered();
        teamy_mft::cli::command::query::QueryArgs::new(GIT_QUERY_PATTERN)
            .invoke()
            .wrap_err("failed querying Teamy MFT index for .git directories")
    });
    let cargo_tomls_task = tokio::task::spawn_blocking(|| {
        let _span = info_span!("query_cargo_toml_files").entered();
        teamy_mft::cli::command::query::QueryArgs::new(CARGO_TOML_QUERY_PATTERN)
            .invoke()
            .wrap_err("failed querying Teamy MFT index for Cargo.toml files")
    });

    let (git_dirs, cargo_tomls) = tokio::try_join!(
        join_query_task(git_dirs_task, ".git directories"),
        join_query_task(cargo_tomls_task, "Cargo.toml files"),
    )?;

    let mut seeds = Vec::with_capacity(git_dirs.len() + cargo_tomls.len());

    for git_dir in git_dirs {
        if is_exact_dot_git_directory(&git_dir) {
            seeds.push(DiscoverySeed::GitDir(git_dir));
        }
    }

    for cargo_toml in cargo_tomls {
        if is_exact_cargo_toml_file(&cargo_toml) {
            seeds.push(DiscoverySeed::CargoToml(cargo_toml));
        }
    }

    debug!(
        candidate_count = seeds.len(),
        "discovered candidate project markers"
    );
    Ok(seeds)
}

async fn join_query_task(
    task: JoinHandle<eyre::Result<Vec<PathBuf>>>,
    description: &'static str,
) -> eyre::Result<Vec<PathBuf>> {
    task.await
        .wrap_err_with(|| format!("Teamy MFT query task failed for {description}"))?
}

fn is_exact_dot_git_directory(path: &Path) -> bool {
    path.is_dir() && file_name_equals_ascii(path, ".git")
}

fn is_exact_cargo_toml_file(path: &Path) -> bool {
    path.is_file() && file_name_equals_ascii(path, "Cargo.toml")
}

fn file_name_equals_ascii(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

// tool[profiling.hot-loop-spans-tracy-gated]
#[cfg_attr(feature = "tracy", tracing::instrument(level = "debug", skip_all))]
fn enrich_seed(seed: DiscoverySeed, config: DiscoveryConfig) -> eyre::Result<PartialProject> {
    match seed {
        DiscoverySeed::GitDir(git_dir) => enrich_git_seed(&git_dir, config),
        DiscoverySeed::CargoToml(cargo_toml) => enrich_cargo_toml_seed(&cargo_toml),
    }
}

// cli[enrichment.gix-repository-metadata]
// cli[enrichment.git-remotes]
// cli[enrichment.git-authors]
// cli[enrichment.git-author-scan-minimum]
// cli[enrichment.git-author-scan-budget]
#[cfg_attr(feature = "tracy", tracing::instrument(level = "debug", skip_all))]
fn enrich_git_seed(git_dir: &Path, config: DiscoveryConfig) -> eyre::Result<PartialProject> {
    let Some(project_dir) = git_dir.parent() else {
        eyre::bail!("git directory has no parent: {}", git_dir.display())
    };

    let mut project = PartialProject {
        path_on_disk: path_to_output_string(project_dir),
        ..Default::default()
    };
    collect_path_name(project_dir, &mut project.names);

    match gix::open(git_dir) {
        Ok(repository) => {
            #[cfg(feature = "tracy")]
            let _span = tracing::debug_span!("collect_git_repository_metadata").entered();
            collect_git_outlinks(&repository, &mut project.outlinks);
            collect_git_authors(&repository, &mut project.authors, config);
            project.newest_branch_activity_at = newest_branch_activity_at(&repository, config.activity_cutoff);
        }
        Err(error) => {
            warn!(path = %project_dir.display(), %error, "failed opening git repository for metadata enrichment");
        }
    }

    Ok(project)
}

fn collect_git_outlinks(repository: &Repository, outlinks: &mut BTreeSet<String>) {
    for remote_name in repository.remote_names() {
        let Ok(remote) = repository.find_remote(remote_name.as_ref()) else {
            continue;
        };

        collect_git_remote_url(remote.url(gix::remote::Direction::Fetch), outlinks);
        collect_git_remote_url(remote.url(gix::remote::Direction::Push), outlinks);
    }
}

fn collect_git_remote_url(url: Option<&gix::Url>, outlinks: &mut BTreeSet<String>) {
    let Some(url) = url else {
        return;
    };

    let url = url.to_bstring();
    if let Some(url) = normalize_non_empty_string(String::from_utf8_lossy(url.as_ref()).as_ref()) {
        outlinks.insert(url);
    }
}

fn collect_git_authors(
    repository: &Repository,
    authors: &mut BTreeSet<String>,
    config: DiscoveryConfig,
) {
    let Ok(head_commit) = repository.head_commit() else {
        return;
    };
    let Ok(mut rev_walk) = repository
        .rev_walk([head_commit.id])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(Default::default()))
        .all()
    else {
        return;
    };

    let started_at = Instant::now();
    let mut scanned_commits = 0usize;

    while should_continue_author_scan(scanned_commits, started_at, config) {
        let chunk_limit = next_author_scan_chunk_limit(scanned_commits, config);
        let mut scanned_in_chunk = 0usize;

        while scanned_in_chunk < chunk_limit {
            let Some(info_result) = rev_walk.next() else {
                return;
            };

            scanned_in_chunk = scanned_in_chunk.saturating_add(1);

            let Ok(info) = info_result else {
                continue;
            };
            let Ok(commit) = info.object() else {
                continue;
            };
            let Ok(commit_ref) = commit.decode() else {
                continue;
            };
            let Ok(author) = commit_ref.author() else {
                continue;
            };

            if let Some(author) = format_gix_signature(author) {
                authors.insert(author);
            }
        }

        scanned_commits = scanned_commits.saturating_add(scanned_in_chunk);
    }
}

// cli[command.surface.activity-filter]
fn newest_branch_activity_at(
    repository: &Repository,
    cutoff: Option<SystemTime>,
) -> Option<SystemTime> {
    let tip_ids = branch_tip_ids(repository);
    if tip_ids.is_empty() {
        return None;
    }

    let sorting = cutoff
        .and_then(system_time_to_unix_seconds)
        .map_or(
            gix::revision::walk::Sorting::ByCommitTime(Default::default()),
            |seconds| gix::revision::walk::Sorting::ByCommitTimeCutoff {
                seconds,
                order: Default::default(),
            },
        );

    let mut walk = repository.rev_walk(tip_ids).sorting(sorting).all().ok()?;
    let newest = walk
        .find_map(Result::ok)
        .and_then(|info| info.commit_time)
        .and_then(unix_seconds_to_system_time);

    match (newest, cutoff) {
        (Some(newest), Some(cutoff)) if newest >= cutoff => Some(newest),
        (Some(_), Some(_)) => None,
        (newest, None) => newest,
        (None, Some(_)) => None,
    }
}

fn branch_tip_ids(repository: &Repository) -> Vec<gix::ObjectId> {
    let mut tip_ids = Vec::new();
    let Ok(references) = repository.references() else {
        return tip_ids;
    };

    collect_branch_tip_ids(references.local_branches().ok(), &mut tip_ids);
    collect_branch_tip_ids(references.remote_branches().ok(), &mut tip_ids);
    tip_ids
}

fn collect_branch_tip_ids<'iter, 'repo>(
    iter: Option<gix::reference::iter::Iter<'iter, 'repo>>,
    tip_ids: &mut Vec<gix::ObjectId>,
) {
    let Some(iter) = iter else {
        return;
    };

    for reference in iter.flatten() {
        let Ok(id) = reference.into_fully_peeled_id() else {
            continue;
        };
        tip_ids.push(id.detach());
    }
}

fn system_time_to_unix_seconds(value: SystemTime) -> Option<gix::date::SecondsSinceUnixEpoch> {
    let duration = value.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_secs()).ok()
}

fn unix_seconds_to_system_time(value: gix::date::SecondsSinceUnixEpoch) -> Option<SystemTime> {
    let seconds = u64::try_from(value).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

fn should_continue_author_scan(
    scanned_commits: usize,
    started_at: Instant,
    config: DiscoveryConfig,
) -> bool {
    scanned_commits < config.author_min_commits || started_at.elapsed() < config.author_scan_budget
}

fn next_author_scan_chunk_limit(scanned_commits: usize, config: DiscoveryConfig) -> usize {
    if scanned_commits < config.author_min_commits {
        config
            .author_min_commits
            .saturating_sub(scanned_commits)
            .min(config.author_scan_chunk_size)
            .max(1)
    } else {
        config.author_scan_chunk_size
    }
}

fn format_gix_signature(signature: gix::actor::SignatureRef<'_>) -> Option<String> {
    format_signature(
        bstr_to_utf8(signature.name),
        bstr_to_utf8(signature.email),
    )
}

fn bstr_to_utf8(value: &gix::bstr::BStr) -> Option<&str> {
    std::str::from_utf8(value.as_ref()).ok()
}

#[cfg_attr(feature = "tracy", tracing::instrument(level = "debug", skip_all))]
fn enrich_cargo_toml_seed(cargo_toml_path: &Path) -> eyre::Result<PartialProject> {
    let Some(project_dir) = cargo_toml_path.parent() else {
        eyre::bail!(
            "Cargo.toml path has no parent: {}",
            cargo_toml_path.display()
        )
    };

    let mut project = PartialProject {
        path_on_disk: path_to_output_string(project_dir),
        ..Default::default()
    };
    collect_path_name(project_dir, &mut project.names);

    let contents = std::fs::read_to_string(cargo_toml_path)
        .wrap_err_with(|| format!("failed reading {}", cargo_toml_path.display()))?;

    match facet_toml::from_str::<CargoManifest>(&contents) {
        Ok(manifest) => {
            if let Some(package) = manifest.package {
                collect_cargo_name_field(package.name.as_deref(), &mut project.names);
                collect_cargo_link_field(package.repository, &mut project.outlinks);
                collect_cargo_link_field(package.homepage, &mut project.outlinks);
                collect_cargo_link_field(package.documentation, &mut project.outlinks);

                if let Some(authors) = package.authors.and_then(CargoStringListField::into_owned) {
                    for author in authors {
                        if let Some(author) = normalize_non_empty_string(&author) {
                            project.authors.insert(author);
                        }
                    }
                }
            }
        }
        Err(error) => {
            if cargo_toml_looks_like_template(&contents) {
                debug!(path = %cargo_toml_path.display(), %error, "skipping templated Cargo.toml during metadata enrichment");
            } else {
                warn!(path = %cargo_toml_path.display(), %error, "failed parsing Cargo.toml for metadata enrichment");
            }
        }
    }

    Ok(project)
}

fn normalize_non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn cargo_toml_looks_like_template(contents: &str) -> bool {
    contents.contains("{{") || contents.contains("}}")
}

fn collect_path_name(path: &Path, names: &mut BTreeSet<String>) {
    if let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str)
        && let Some(name) = normalize_non_empty_string(name)
    {
        names.insert(name);
    }
}

fn collect_cargo_name_field(field: Option<&str>, names: &mut BTreeSet<String>) {
    if let Some(name) = field.and_then(normalize_non_empty_string) {
        names.insert(name);
    }
}

fn collect_cargo_link_field(field: Option<CargoStringField>, outlinks: &mut BTreeSet<String>) {
    if let Some(link) = field.and_then(CargoStringField::into_owned)
        && let Some(link) = normalize_non_empty_string(&link)
    {
        outlinks.insert(link);
    }
}

impl CargoStringField {
    fn into_owned(self) -> Option<String> {
        match self {
            Self::Value(value) => Some(value),
            Self::Workspace(_) => None,
        }
    }
}

impl CargoStringListField {
    fn into_owned(self) -> Option<Vec<String>> {
        match self {
            Self::Value(values) => Some(values),
            Self::Workspace(_) => None,
        }
    }
}

fn format_signature(name: Option<&str>, email: Option<&str>) -> Option<String> {
    let name = name.and_then(normalize_non_empty_string);
    let email = email.and_then(normalize_non_empty_string);

    match (name, email) {
        (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
        (Some(name), None) => Some(name),
        (None, Some(email)) => Some(format!("<{email}>")),
        (None, None) => None,
    }
}

fn activity_output_fields(
    activity_at: Option<SystemTime>,
    now: SystemTime,
) -> (Option<String>, Option<String>) {
    let Some(activity_at) = activity_at else {
        return (None, None);
    };

    (
        Some(humantime::format_rfc3339_seconds(activity_at).to_string()),
        Some(format_relative_activity(activity_at, now)),
    )
}

fn format_relative_activity(activity_at: SystemTime, now: SystemTime) -> String {
    match now.duration_since(activity_at) {
        Ok(duration) => format!("{} ago", format_human_duration(duration)),
        Err(_) => match activity_at.duration_since(now) {
            Ok(duration) => format!("in {}", format_human_duration(duration)),
            Err(_) => "0s ago".to_owned(),
        },
    }
}

fn format_human_duration(duration: Duration) -> String {
    humantime::format_duration(Duration::from_secs(duration.as_secs())).to_string()
}

fn merge_partials(
    partials: Vec<PartialProject>,
    activity_cutoff: Option<SystemTime>,
) -> Vec<DiscoveredProjectRecord> {
    let mut merged = BTreeMap::<String, PartialProject>::new();

    for partial in partials {
        let key = project_key(&partial.path_on_disk);
        let entry = merged.entry(key).or_insert_with(|| PartialProject {
            path_on_disk: partial.path_on_disk.clone(),
            ..Default::default()
        });
        entry.names.extend(partial.names);
        entry.outlinks.extend(partial.outlinks);
        entry.authors.extend(partial.authors);
        entry.newest_branch_activity_at = match (
            entry.newest_branch_activity_at,
            partial.newest_branch_activity_at,
        ) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        };
    }

    merged
        .into_values()
        .filter(|partial| {
            activity_cutoff.is_none_or(|cutoff| {
                partial
                    .newest_branch_activity_at
                    .is_some_and(|activity| activity >= cutoff)
            })
        })
        .map(|partial| DiscoveredProjectRecord {
            project: DiscoveredProject {
                path_on_disk: partial.path_on_disk,
                names: partial.names.into_iter().collect(),
                outlinks: partial.outlinks.into_iter().collect(),
                authors: partial.authors.into_iter().collect(),
                last_activity_on: None,
                last_activity_ago: None,
            },
            newest_branch_activity_at: partial.newest_branch_activity_at,
        })
        .collect()
}

fn project_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}

fn path_to_output_string(path: &Path) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::CargoManifest;
    use super::CargoStringField;
    use super::CargoStringListField;
    use super::DiscoveredProject;
    use super::DiscoveredProjectRecord;
    use super::DiscoveryConfig;
    use super::PartialProject;
    use super::WorkspaceField;
    use super::activity_output_fields;
    use super::format_signature;
    use super::format_human_duration;
    use super::format_relative_activity;
    use super::merge_partials;
    use super::next_author_scan_chunk_limit;
    use super::project_key;
    use super::should_continue_author_scan;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::time::Duration;
    use std::time::Instant;
    use std::time::SystemTime;

    #[test]
    fn parses_cargo_manifest_name_repository_and_authors() {
        let manifest: CargoManifest = facet_toml::from_str(
            r#"
                [package]
                name = "example-repo"
                repository = "https://github.com/example/repo"
                homepage = "https://example.com/project"
                documentation = "https://docs.rs/example"
                authors = ["Ada <ada@example.com>", "Grace <grace@example.com>"]
            "#,
        )
        .expect("cargo manifest should parse");

        let package = manifest.package.expect("package table should exist");
        assert_eq!(package.name.as_deref(), Some("example-repo"));
        assert_eq!(
            package.repository,
            Some(CargoStringField::Value(
                "https://github.com/example/repo".to_owned()
            ))
        );
        assert_eq!(
            package.homepage,
            Some(CargoStringField::Value(
                "https://example.com/project".to_owned()
            ))
        );
        assert_eq!(
            package.documentation,
            Some(CargoStringField::Value(
                "https://docs.rs/example".to_owned()
            ))
        );
        assert_eq!(
            package.authors,
            Some(CargoStringListField::Value(vec![
                "Ada <ada@example.com>".to_owned(),
                "Grace <grace@example.com>".to_owned(),
            ]))
        );
    }

    #[test]
    fn parses_workspace_inherited_cargo_manifest_metadata() {
        let manifest: CargoManifest = facet_toml::from_str(
            r#"
                [package]
                repository.workspace = true
                homepage.workspace = true
                documentation.workspace = true
                authors.workspace = true
            "#,
        )
        .expect("cargo manifest should parse workspace metadata");

        let package = manifest.package.expect("package table should exist");
        assert_eq!(
            package.repository,
            Some(CargoStringField::Workspace(WorkspaceField {
                workspace: true,
            }))
        );
        assert_eq!(
            package.homepage,
            Some(CargoStringField::Workspace(WorkspaceField {
                workspace: true,
            }))
        );
        assert_eq!(
            package.documentation,
            Some(CargoStringField::Workspace(WorkspaceField {
                workspace: true,
            }))
        );
        assert_eq!(
            package.authors,
            Some(CargoStringListField::Workspace(WorkspaceField {
                workspace: true,
            }))
        );
    }

    #[test]
    fn collects_all_cargo_link_fields_into_outlinks() {
        let mut outlinks = BTreeSet::new();

        super::collect_cargo_link_field(
            Some(CargoStringField::Value(
                "https://github.com/example/repo".to_owned(),
            )),
            &mut outlinks,
        );
        super::collect_cargo_link_field(
            Some(CargoStringField::Value(
                "https://example.com/project".to_owned(),
            )),
            &mut outlinks,
        );
        super::collect_cargo_link_field(
            Some(CargoStringField::Workspace(WorkspaceField {
                workspace: true,
            })),
            &mut outlinks,
        );

        assert_eq!(
            outlinks.into_iter().collect::<Vec<_>>(),
            vec![
                "https://example.com/project".to_owned(),
                "https://github.com/example/repo".to_owned(),
            ]
        );
    }

    #[test]
    fn collects_path_and_cargo_names_into_names() {
        let mut names = BTreeSet::new();

        super::collect_path_name(Path::new(r"C:\Repos\example-repo"), &mut names);
        super::collect_cargo_name_field(Some("crate-name"), &mut names);

        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["crate-name".to_owned(), "example-repo".to_owned()]
        );
    }

    #[test]
    fn detects_template_placeholders_in_manifest_contents() {
        assert!(super::cargo_toml_looks_like_template(
            "name = \"{{crate_name}}\""
        ));
        assert!(!super::cargo_toml_looks_like_template(
            "name = \"normal-package\""
        ));
    }

    #[test]
    fn merges_metadata_for_same_project_path() {
        let mut first = PartialProject {
            path_on_disk: r"C:\Repo".to_owned(),
            ..Default::default()
        };
        first.names = BTreeSet::from(["Repo".to_owned()]);
        first.outlinks = BTreeSet::from(["https://example.com/one".to_owned()]);
        first.authors = BTreeSet::from(["Ada <ada@example.com>".to_owned()]);

        let mut second = PartialProject {
            path_on_disk: r"C:\Repo".to_owned(),
            ..Default::default()
        };
        second.names = BTreeSet::from(["repo-crate".to_owned()]);
        second.outlinks = BTreeSet::from([
            "https://example.com/one".to_owned(),
            "https://example.com/two".to_owned(),
        ]);
        second.authors = BTreeSet::from(["Grace <grace@example.com>".to_owned()]);

        let merged = merge_partials(vec![first, second], None);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].project.path_on_disk, r"C:\Repo");
        assert_eq!(
            merged[0].project.names,
            vec!["Repo".to_owned(), "repo-crate".to_owned()]
        );
        assert_eq!(
            merged[0].project.outlinks,
            vec![
                "https://example.com/one".to_owned(),
                "https://example.com/two".to_owned(),
            ]
        );
        assert_eq!(
            merged[0].project.authors,
            vec![
                "Ada <ada@example.com>".to_owned(),
                "Grace <grace@example.com>".to_owned(),
            ]
        );
    }

    #[test]
    fn formats_git_signature_like_git() {
        assert_eq!(
            format_signature(Some("Ada"), Some("ada@example.com")),
            Some("Ada <ada@example.com>".to_owned())
        );
        assert_eq!(format_signature(Some("Ada"), None), Some("Ada".to_owned()));
        assert_eq!(
            format_signature(None, Some("ada@example.com")),
            Some("<ada@example.com>".to_owned())
        );
        assert_eq!(format_signature(None, None), None);
    }

    #[test]
    fn project_key_is_case_insensitive_on_windows() {
        if cfg!(windows) {
            assert_eq!(project_key(r"C:\Repo"), project_key(r"c:\repo"));
        }
    }

    #[test]
    fn discovery_config_treats_zero_max_in_flight_as_unbounded() {
        let config = DiscoveryConfig::with_overrides(Some(0), Some(8), Some(250), Some(0), None);

        assert_eq!(config.max_in_flight, None);
        assert_eq!(config.author_min_commits, 8);
        assert_eq!(config.author_scan_budget, Duration::from_millis(250));
        assert_eq!(config.author_scan_chunk_size, 1);
        assert_eq!(config.activity_cutoff, None);
    }

    #[test]
    fn author_scan_continues_until_minimum_commits_are_scanned() {
        let config = DiscoveryConfig {
            max_in_flight: None,
            author_min_commits: 4,
            author_scan_budget: Duration::ZERO,
            author_scan_chunk_size: 2,
            activity_cutoff: None,
        };
        let started_at = Instant::now() - Duration::from_secs(1);

        assert!(should_continue_author_scan(0, started_at, config));
        assert!(should_continue_author_scan(3, started_at, config));
        assert!(!should_continue_author_scan(4, started_at, config));
    }

    #[test]
    fn author_scan_uses_budget_after_minimum_commits_are_scanned() {
        let config = DiscoveryConfig {
            max_in_flight: None,
            author_min_commits: 2,
            author_scan_budget: Duration::from_secs(5),
            author_scan_chunk_size: 3,
            activity_cutoff: None,
        };

        assert!(should_continue_author_scan(2, Instant::now(), config));
        assert!(!should_continue_author_scan(
            2,
            Instant::now() - Duration::from_secs(10),
            config,
        ));
    }

    #[test]
    fn author_scan_chunk_limit_finishes_minimum_before_regular_chunks() {
        let config = DiscoveryConfig {
            max_in_flight: None,
            author_min_commits: 5,
            author_scan_budget: Duration::from_secs(1),
            author_scan_chunk_size: 3,
            activity_cutoff: None,
        };

        assert_eq!(next_author_scan_chunk_limit(0, config), 3);
        assert_eq!(next_author_scan_chunk_limit(3, config), 2);
        assert_eq!(next_author_scan_chunk_limit(5, config), 3);
    }

    #[test]
    fn merge_partials_keeps_newest_branch_activity() {
        let newer = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let older = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        let first = PartialProject {
            path_on_disk: r"C:\Repo".to_owned(),
            newest_branch_activity_at: Some(older),
            ..Default::default()
        };
        let second = PartialProject {
            path_on_disk: r"C:\Repo".to_owned(),
            newest_branch_activity_at: Some(newer),
            ..Default::default()
        };

        let merged = merge_partials(vec![first, second], None);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].newest_branch_activity_at, Some(newer));
    }

    #[test]
    fn merge_partials_applies_activity_cutoff() {
        let newer = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let older = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        let recent = PartialProject {
            path_on_disk: r"C:\Recent".to_owned(),
            newest_branch_activity_at: Some(newer),
            ..Default::default()
        };
        let stale = PartialProject {
            path_on_disk: r"C:\Stale".to_owned(),
            newest_branch_activity_at: Some(older),
            ..Default::default()
        };
        let cargo_only = PartialProject {
            path_on_disk: r"C:\CargoOnly".to_owned(),
            ..Default::default()
        };

        let merged = merge_partials(
            vec![recent, stale, cargo_only],
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(15)),
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].project.path_on_disk, r"C:\Recent");
    }

    #[test]
    fn project_record_formats_activity_fields_for_output() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400 * 3);
        let activity_at = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400 * 2);
        let expected_ago = format!(
            "{} ago",
            humantime::format_duration(Duration::from_secs(86_400))
        );
        let project = DiscoveredProjectRecord {
            project: DiscoveredProject {
                path_on_disk: r"C:\Repo".to_owned(),
                ..Default::default()
            },
            newest_branch_activity_at: Some(activity_at),
        }
        .into_project_at(now);

        assert_eq!(project.last_activity_on.as_deref(), Some("1970-01-03T00:00:00Z"));
        assert_eq!(project.last_activity_ago.as_deref(), Some(expected_ago.as_str()));
    }

    #[test]
    fn activity_output_fields_are_null_when_no_activity_is_known() {
        let project = DiscoveredProject {
            path_on_disk: r"C:\Repo".to_owned(),
            ..Default::default()
        };

        let json = facet_json::to_string_pretty(&project).expect("project should serialize");

        assert!(json.contains("\"last_activity_on\": null"));
        assert!(json.contains("\"last_activity_ago\": null"));
    }

    #[test]
    fn activity_output_fields_return_none_without_timestamp() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(5);

        assert_eq!(activity_output_fields(None, now), (None, None));
    }

    #[test]
    fn relative_activity_formats_future_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let activity_at = SystemTime::UNIX_EPOCH + Duration::from_secs(25);

        assert_eq!(
            format_relative_activity(activity_at, now),
            format!("in {}", humantime::format_duration(Duration::from_secs(15)))
        );
    }

    #[test]
    fn human_duration_drops_subsecond_noise() {
        assert_eq!(
            format_human_duration(Duration::from_millis(1_999)),
            humantime::format_duration(Duration::from_secs(1)).to_string()
        );
    }
}
