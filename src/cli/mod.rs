pub mod facet_shape;
pub mod global_args;

use crate::cli::global_args::GlobalArgs;
use arbitrary::Arbitrary;
use eyre::Context;
use facet::Facet;
use figue::FigueBuiltins;

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

    /// Standard CLI options (help, version, completions).
    #[facet(flatten)]
    #[arbitrary(default)]
    pub builtins: FigueBuiltins,
}

impl PartialEq for Cli {
    fn eq(&self, other: &Self) -> bool {
        // Ignore builtins in comparison since FigueBuiltins doesn't implement PartialEq
        self.global_args == other.global_args
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
            let projects = crate::discovery::discover_projects().await?;
            let json = facet_json::to_string_pretty(&projects)
                .wrap_err("Failed to serialize discovery results as JSON")?;
            println!("{json}");
            eyre::Result::<()>::Ok(())
        })?;
        Ok(())
    }
}
