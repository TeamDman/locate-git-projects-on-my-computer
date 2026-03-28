pub mod facet_shape;
pub mod global_args;

use crate::cli::global_args::GlobalArgs;
use arbitrary::Arbitrary;
use eyre::Context;
use facet::Facet;
use figue::FigueBuiltins;
use figue::{self as args};

/// Locate source projects on disk and emit structured discovery results.
///
/// Environment variables:
/// - `LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_HOME_DIR` overrides the resolved application home directory.
/// - `LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_CACHE_DIR` overrides the resolved cache directory.
/// - `TEAMY_MFT_SYNC_DIR` overrides the Teamy MFT sync directory used for indexed discovery.
/// - `RUST_LOG` provides a tracing filter when `--log-filter` is omitted.
#[derive(Facet, Arbitrary, Debug)]
pub struct Cli {
    /// Global arguments (`debug`, `log_filter`, `log_file`).
    #[facet(flatten)]
    pub global_args: GlobalArgs,

    /// Keep projects whose discovered names contain at least one provided value.
    #[facet(args::named, default)]
    pub name: Vec<String>,

    /// Keep projects whose discovered authors contain at least one provided value.
    #[facet(args::named, default)]
    pub author: Vec<String>,

    /// Keep projects whose discovered outlinks contain at least one provided value.
    #[facet(args::named, default)]
    pub url: Vec<String>,

    /// Standard CLI options (help, version, completions).
    #[facet(flatten)]
    #[arbitrary(default)]
    pub builtins: FigueBuiltins,
}

impl PartialEq for Cli {
    fn eq(&self, other: &Self) -> bool {
        // Ignore builtins in comparison since FigueBuiltins doesn't implement PartialEq
        self.global_args == other.global_args
            && self.name == other.name
            && self.author == other.author
            && self.url == other.url
    }
}

impl Cli {
    /// # Errors
    ///
    /// This function will return an error if the tokio runtime cannot be built or if the command fails.
    pub fn invoke(self) -> eyre::Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .wrap_err("Failed to build tokio runtime")?;
        runtime.block_on(async move {
            let projects = crate::discovery::discover_projects()
                .await?
                .into_iter()
                .filter(|project| matches_any_filter(project, &self.name, &self.author, &self.url))
                .collect::<Vec<_>>();
            let json = facet_json::to_string_pretty(&projects)
                .wrap_err("Failed to serialize discovery results as JSON")?;
            println!("{json}");
            eyre::Result::<()>::Ok(())
        })?;
        Ok(())
    }
}

fn matches_any_filter(
    project: &crate::discovery::DiscoveredProject,
    names: &[String],
    authors: &[String],
    urls: &[String],
) -> bool {
    let has_filters = !(names.is_empty() && authors.is_empty() && urls.is_empty());
    if !has_filters {
        return true;
    }

    contains_any_name(&project.names, names)
        || contains_any(&project.authors, authors)
        || contains_any(&project.outlinks, urls)
}

fn contains_any(values: &[String], filters: &[String]) -> bool {
    filters.iter().any(|filter| values.contains(filter))
}

fn contains_any_name(values: &[String], filters: &[String]) -> bool {
    filters.iter().any(|filter| {
        let normalized_filter = normalize_name_for_matching(filter);
        values
            .iter()
            .any(|value| normalize_name_for_matching(value) == normalized_filter)
    })
}

fn normalize_name_for_matching(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '-' | '_' => '_',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::contains_any;
    use super::contains_any_name;
    use super::matches_any_filter;
    use super::normalize_name_for_matching;
    use crate::discovery::DiscoveredProject;

    #[test]
    fn unfiltered_cli_keeps_all_projects() {
        let project = DiscoveredProject {
            path_on_disk: String::new(),
            names: vec!["repo".to_owned()],
            outlinks: vec!["https://example.com".to_owned()],
            authors: vec!["Ada <ada@example.com>".to_owned()],
        };

        assert!(matches_any_filter(&project, &[], &[], &[]));
    }

    #[test]
    fn filters_match_when_any_requested_value_is_present() {
        let project = DiscoveredProject {
            path_on_disk: String::new(),
            names: vec!["repo_name".to_owned()],
            outlinks: vec!["https://example.com".to_owned()],
            authors: vec!["Ada <ada@example.com>".to_owned()],
        };

        assert!(matches_any_filter(
            &project,
            &["repo-name".to_owned()],
            &[],
            &[]
        ));
        assert!(matches_any_filter(
            &project,
            &[],
            &["Ada <ada@example.com>".to_owned()],
            &[]
        ));
        assert!(matches_any_filter(
            &project,
            &[],
            &[],
            &["https://example.com".to_owned()]
        ));
        assert!(matches_any_filter(
            &project,
            &["missing".to_owned()],
            &["Ada <ada@example.com>".to_owned()],
            &[]
        ));
    }

    #[test]
    fn filters_reject_projects_without_any_requested_values() {
        let project = DiscoveredProject {
            path_on_disk: String::new(),
            names: vec!["repo_name".to_owned()],
            outlinks: vec!["https://example.com".to_owned()],
            authors: vec!["Ada <ada@example.com>".to_owned()],
        };

        assert!(!matches_any_filter(
            &project,
            &["missing".to_owned()],
            &["Unknown <unknown@example.com>".to_owned()],
            &["https://other.example.com".to_owned()]
        ));
    }

    #[test]
    fn contains_any_uses_exact_membership() {
        assert!(contains_any(&["repo".to_owned()], &["repo".to_owned()]));
        assert!(!contains_any(&["repo".to_owned()], &["rep".to_owned()]));
    }

    #[test]
    fn contains_any_name_normalizes_dashes_and_underscores() {
        assert!(contains_any_name(
            &["rc-zip".to_owned()],
            &["rc_zip".to_owned()]
        ));
        assert!(contains_any_name(
            &["rc_zip".to_owned()],
            &["rc-zip".to_owned()]
        ));
        assert!(!contains_any_name(
            &["rczip".to_owned()],
            &["rc_zip".to_owned()]
        ));
    }

    #[test]
    fn normalize_name_for_matching_only_rewrites_dash_and_underscore() {
        assert_eq!(normalize_name_for_matching("rc-zip"), "rc_zip");
        assert_eq!(normalize_name_for_matching("rc_zip"), "rc_zip");
        assert_eq!(normalize_name_for_matching("Rc.Zip"), "Rc.Zip");
    }
}
