pub mod facet_shape;
pub mod global_args;

use crate::cli::global_args::GlobalArgs;
use crate::discovery::DiscoveryConfig;
use crate::discovery::DiscoveredProjectRecord;
use arbitrary::Arbitrary;
use eyre::Context;
use facet::Facet;
use figue::FigueBuiltins;
use figue::{self as args};
use std::time::SystemTime;

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

    /// Keep projects whose discovered authors case-insensitively contain at least one provided value.
    #[facet(args::named, default)]
    pub author: Vec<String>,

    /// Keep projects whose discovered outlinks contain at least one provided value.
    #[facet(args::named, default)]
    pub url: Vec<String>,

    /// Keep git repositories whose branch activity contains a commit newer than now minus the provided humantime duration.
    #[facet(args::named)]
    pub activity: Option<String>,

    // cli[command.surface.discovery-tuning-flags]
    /// Maximum number of metadata-enrichment tasks to allow in flight.
    ///
    /// If omitted, discovery uses the repository default.
    /// If set to `0`, discovery spawns an enrichment task per candidate and lets Tokio's blocking pool schedule them.
    #[facet(args::named)]
    pub enrichment_max_in_flight: Option<usize>,

    // cli[command.surface.discovery-tuning-flags]
    /// Minimum number of reachable commits to scan for authors before time budgeting can stop the walk.
    ///
    /// If omitted, discovery uses its default.
    #[facet(args::named)]
    pub author_min_commits: Option<usize>,

    // cli[command.surface.discovery-tuning-flags]
    /// Maximum wall-clock time in milliseconds to spend scanning authors for one repository after the minimum scan depth is satisfied.
    ///
    /// If omitted, discovery uses its default. A value of `0` stops immediately after the minimum scan depth is reached.
    #[facet(args::named)]
    pub author_scan_budget_ms: Option<u64>,

    // cli[command.surface.discovery-tuning-flags]
    /// Number of commits to scan between time-budget checks while gathering authors.
    ///
    /// If omitted, discovery uses its default.
    #[facet(args::named)]
    pub author_scan_chunk_size: Option<usize>,

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
            && self.activity == other.activity
            && self.enrichment_max_in_flight == other.enrichment_max_in_flight
            && self.author_min_commits == other.author_min_commits
            && self.author_scan_budget_ms == other.author_scan_budget_ms
            && self.author_scan_chunk_size == other.author_scan_chunk_size
    }
}

impl Cli {
    fn discovery_config(&self) -> DiscoveryConfig {
        DiscoveryConfig::with_overrides(
            self.enrichment_max_in_flight,
            self.author_min_commits,
            self.author_scan_budget_ms,
            self.author_scan_chunk_size,
            activity_cutoff_from_now(self.activity.as_deref(), SystemTime::now()),
        )
    }

    /// # Errors
    ///
    /// This function will return an error if the tokio runtime cannot be built or if the command fails.
    pub fn invoke(self) -> eyre::Result<()> {
        let discovery_config = self.discovery_config();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .wrap_err("Failed to build tokio runtime")?;
        runtime.block_on(async move {
            let now = SystemTime::now();
            let projects = crate::discovery::discover_project_records_with_config(discovery_config)
                .await?
                .into_iter()
                .filter(|record| matches_any_filter(record, &self.name, &self.author, &self.url))
                .map(|record| record.into_project_at(now))
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
    record: &DiscoveredProjectRecord,
    names: &[String],
    authors: &[String],
    urls: &[String],
) -> bool {
    let has_filters = !(names.is_empty() && authors.is_empty() && urls.is_empty());
    if !has_filters {
        return true;
    }

    contains_any_name(&record.project.names, names)
        || contains_any_case_insensitive_substring(&record.project.authors, authors)
        || contains_any(&record.project.outlinks, urls)
}

fn activity_cutoff_from_now(value: Option<&str>, now: SystemTime) -> Option<SystemTime> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    humantime::parse_duration(value)
        .ok()
        .and_then(|duration| now.checked_sub(duration))
}

fn contains_any(values: &[String], filters: &[String]) -> bool {
    filters.iter().any(|filter| values.contains(filter))
}

// cli[command.surface.author-filter-case-insensitive-substring]
fn contains_any_case_insensitive_substring(values: &[String], filters: &[String]) -> bool {
    filters
        .iter()
        .filter_map(|filter| {
            let trimmed = filter.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_lowercase())
        })
        .any(|filter| {
            values
                .iter()
                .any(|value| value.to_lowercase().contains(&filter))
        })
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
    use super::activity_cutoff_from_now;
    use super::contains_any;
    use super::contains_any_case_insensitive_substring;
    use super::contains_any_name;
    use super::matches_any_filter;
    use super::normalize_name_for_matching;
    use crate::discovery::DiscoveredProjectRecord;
    use crate::discovery::DiscoveredProject;
    use std::time::Duration;
    use std::time::SystemTime;

    #[test]
    fn unfiltered_cli_keeps_all_projects() {
        let project = sample_record();

        assert!(matches_any_filter(&project, &[], &[], &[]));
    }

    #[test]
    fn filters_match_when_any_requested_value_is_present() {
        let project = sample_record();

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
        assert!(matches_any_filter(&project, &[], &["ada".to_owned()], &[]));
        assert!(matches_any_filter(
            &project,
            &[],
            &["EXAMPLE.COM".to_owned()],
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
        let project = sample_record();

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
    fn author_filters_use_case_insensitive_substring_matching() {
        assert!(contains_any_case_insensitive_substring(
            &["Ada Lovelace <ada@example.com>".to_owned()],
            &["lovelace".to_owned()]
        ));
        assert!(contains_any_case_insensitive_substring(
            &["Ada Lovelace <ada@example.com>".to_owned()],
            &["ADA@EXAMPLE".to_owned()]
        ));
        assert!(!contains_any_case_insensitive_substring(
            &["Ada Lovelace <ada@example.com>".to_owned()],
            &["grace".to_owned()]
        ));
        assert!(!contains_any_case_insensitive_substring(
            &["Ada Lovelace <ada@example.com>".to_owned()],
            &["   ".to_owned()]
        ));
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

    #[test]
    fn activity_cutoff_parses_humantime_relative_to_now() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400 * 5);
        let cutoff = activity_cutoff_from_now(Some("1day"), now)
            .expect("humantime value should parse");

        assert_eq!(cutoff, now - Duration::from_secs(86_400));
    }

    #[test]
    fn activity_cutoff_ignores_empty_or_invalid_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        assert_eq!(activity_cutoff_from_now(Some("   "), now), None);
        assert_eq!(activity_cutoff_from_now(Some("not-a-duration"), now), None);
        assert_eq!(activity_cutoff_from_now(None, now), None);
    }

    fn sample_record() -> DiscoveredProjectRecord {
        DiscoveredProjectRecord {
            project: DiscoveredProject {
                path_on_disk: String::new(),
                names: vec!["repo_name".to_owned()],
                outlinks: vec!["https://example.com".to_owned()],
                authors: vec!["Ada <ada@example.com>".to_owned()],
                last_activity_on: None,
                last_activity_ago: None,
            },
            newest_branch_activity_at: None,
        }
    }
}
