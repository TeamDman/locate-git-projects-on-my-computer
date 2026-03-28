use eyre::Context;
use facet::Facet;
use git2::Repository;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use tokio::task::JoinSet;
use tracing::debug;
use tracing::warn;

const GIT_QUERY_PATTERN: &str = ".git$";
const CARGO_TOML_QUERY_PATTERN: &str = "Cargo.toml$";
const MAX_GIT_COMMITS_TO_SCAN: usize = 10_000;

/// A single source project found on disk.
#[derive(Facet, Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveredProject {
    /// The on-disk path for this specific discovered project entry.
    pub path_on_disk: String,
    /// Source repository or file URIs associated with this entry.
    pub outlinks: Vec<String>,
    /// Authors gathered from repository history or package metadata.
    pub authors: Vec<String>,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq, Default)]
struct CargoManifest {
    package: Option<CargoPackage>,
}

#[derive(Facet, Debug, Clone, PartialEq, Eq, Default)]
struct CargoPackage {
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
    outlinks: BTreeSet<String>,
    authors: BTreeSet<String>,
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
    let seeds = discover_candidate_paths()?;
    let mut partials = Vec::new();
    let mut join_set = JoinSet::new();
    let max_in_flight = std::thread::available_parallelism()
        .map(|value| value.get().saturating_mul(4))
        .unwrap_or(16)
        .max(1);

    for seed in seeds {
        if join_set.len() >= max_in_flight {
            partials.push(join_next_partial(&mut join_set).await?);
        }

        join_set.spawn_blocking(move || enrich_seed(seed));
    }

    while !join_set.is_empty() {
        partials.push(join_next_partial(&mut join_set).await?);
    }

    Ok(merge_partials(partials))
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

fn discover_candidate_paths() -> eyre::Result<Vec<DiscoverySeed>> {
    let git_dirs = teamy_mft::cli::command::query::QueryArgs::new(GIT_QUERY_PATTERN)
        .invoke()
        .wrap_err("failed querying Teamy MFT index for .git directories")?;
    let cargo_tomls = teamy_mft::cli::command::query::QueryArgs::new(CARGO_TOML_QUERY_PATTERN)
        .invoke()
        .wrap_err("failed querying Teamy MFT index for Cargo.toml files")?;

    let mut seeds = Vec::new();

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

fn enrich_seed(seed: DiscoverySeed) -> eyre::Result<PartialProject> {
    match seed {
        DiscoverySeed::GitDir(git_dir) => enrich_git_seed(&git_dir),
        DiscoverySeed::CargoToml(cargo_toml) => enrich_cargo_toml_seed(&cargo_toml),
    }
}

fn enrich_git_seed(git_dir: &Path) -> eyre::Result<PartialProject> {
    let Some(project_dir) = git_dir.parent() else {
        eyre::bail!("git directory has no parent: {}", git_dir.display())
    };

    let mut project = PartialProject {
        path_on_disk: path_to_output_string(project_dir),
        ..Default::default()
    };

    match Repository::open(project_dir) {
        Ok(repository) => {
            collect_git_outlinks(&repository, &mut project.outlinks);
            collect_git_authors(&repository, &mut project.authors);
        }
        Err(error) => {
            warn!(path = %project_dir.display(), %error, "failed opening git repository for metadata enrichment");
        }
    }

    Ok(project)
}

fn collect_git_outlinks(repository: &Repository, outlinks: &mut BTreeSet<String>) {
    let Ok(remotes) = repository.remotes() else {
        return;
    };

    for remote_name in remotes.iter().flatten() {
        let Ok(remote) = repository.find_remote(remote_name) else {
            continue;
        };

        if let Some(url) = remote
            .url()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            outlinks.insert(url.to_owned());
        }

        if let Some(url) = remote
            .pushurl()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            outlinks.insert(url.to_owned());
        }
    }
}

fn collect_git_authors(repository: &Repository, authors: &mut BTreeSet<String>) {
    let Ok(mut revwalk) = repository.revwalk() else {
        return;
    };
    if revwalk.push_head().is_err() {
        return;
    }

    for oid in revwalk.take(MAX_GIT_COMMITS_TO_SCAN).flatten() {
        let Ok(commit) = repository.find_commit(oid) else {
            continue;
        };

        if let Some(author) = format_signature(commit.author().name(), commit.author().email()) {
            authors.insert(author);
        }
    }
}

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

    let contents = std::fs::read_to_string(cargo_toml_path)
        .wrap_err_with(|| format!("failed reading {}", cargo_toml_path.display()))?;

    match facet_toml::from_str::<CargoManifest>(&contents) {
        Ok(manifest) => {
            if let Some(package) = manifest.package {
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

fn merge_partials(partials: Vec<PartialProject>) -> Vec<DiscoveredProject> {
    let mut merged = BTreeMap::<String, PartialProject>::new();

    for partial in partials {
        let key = project_key(&partial.path_on_disk);
        let entry = merged.entry(key).or_insert_with(|| PartialProject {
            path_on_disk: partial.path_on_disk.clone(),
            ..Default::default()
        });
        entry.outlinks.extend(partial.outlinks);
        entry.authors.extend(partial.authors);
    }

    merged
        .into_values()
        .map(|partial| DiscoveredProject {
            path_on_disk: partial.path_on_disk,
            outlinks: partial.outlinks.into_iter().collect(),
            authors: partial.authors.into_iter().collect(),
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
    use super::PartialProject;
    use super::WorkspaceField;
    use super::format_signature;
    use super::merge_partials;
    use super::project_key;
    use std::collections::BTreeSet;

    #[test]
    fn parses_cargo_manifest_repository_and_authors() {
        let manifest: CargoManifest = facet_toml::from_str(
            r#"
                [package]
                repository = "https://github.com/example/repo"
                homepage = "https://example.com/project"
                documentation = "https://docs.rs/example"
                authors = ["Ada <ada@example.com>", "Grace <grace@example.com>"]
            "#,
        )
        .expect("cargo manifest should parse");

        let package = manifest.package.expect("package table should exist");
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
        first.outlinks = BTreeSet::from(["https://example.com/one".to_owned()]);
        first.authors = BTreeSet::from(["Ada <ada@example.com>".to_owned()]);

        let mut second = PartialProject {
            path_on_disk: r"C:\Repo".to_owned(),
            ..Default::default()
        };
        second.outlinks = BTreeSet::from([
            "https://example.com/one".to_owned(),
            "https://example.com/two".to_owned(),
        ]);
        second.authors = BTreeSet::from(["Grace <grace@example.com>".to_owned()]);

        let merged = merge_partials(vec![first, second]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].path_on_disk, r"C:\Repo");
        assert_eq!(
            merged[0].outlinks,
            vec![
                "https://example.com/one".to_owned(),
                "https://example.com/two".to_owned(),
            ]
        );
        assert_eq!(
            merged[0].authors,
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
}
