#![deny(clippy::disallowed_methods)]
#![deny(clippy::disallowed_macros)]

pub mod cli;
pub mod discovery;
pub mod logging_init;
pub mod paths;
#[cfg(windows)]
mod windows_startup;

use crate::cli::Cli;
use chrono::{DateTime, Local, Utc};

/// Version string components embedded by Cargo and the build script.
pub const APP_SEMVER: &str = env!("CARGO_PKG_VERSION");
pub const APP_GIT_REVISION: &str = env!("GIT_REVISION");
pub const APP_BUILD_UNIX_MS: &str = env!("BUILD_UNIX_MS");

fn version() -> String {
    let built_at = APP_BUILD_UNIX_MS
        .parse::<i64>()
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map_or_else(
            || String::from("unknown build time"),
            |timestamp| {
                timestamp
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S %Z")
                    .to_string()
            },
        );

    format!("{APP_SEMVER} (rev {APP_GIT_REVISION}, built {built_at})")
}

/// Entrypoint for the program.
///
/// # Errors
///
/// This function will return an error if `color_eyre` installation, CLI parsing, logging initialization, or command execution fails.
///
/// # Panics
///
/// Panics if the CLI schema is invalid (should never happen with correct code).
pub fn main() -> eyre::Result<()> {
    // Install color_eyre for better error reports
    color_eyre::install()?;

    #[cfg(windows)]
    {
        // Enable ANSI support on Windows
        // This fails in a pipe scenario, so we ignore the error
        let _ = windows_startup::enable_ansi_support();

        // Warn if UTF-8 is not enabled on Windows
        #[cfg(windows)]
        windows_startup::warn_if_utf8_not_enabled();
    };

    // Parse command line arguments using figue
    // unwrap() is figue's intended CLI entry behavior:
    // it exits with proper codes for --help/--version/completions/parse-errors.
    let version = version();
    let cli: Cli = figue::Driver::new(
        figue::builder::<Cli>()
            .expect("schema should be valid")
            .cli(move |cli| cli.args_os(std::env::args_os().skip(1)).strict())
            .help(move |help| {
                help.version(version)
                    .include_implementation_source_file(true)
                    .include_implementation_git_url(
                        "TeamDman/locate-git-projects-on-my-computer",
                        env!("GIT_REVISION"),
                    )
            })
            .build(),
    )
    .run()
    .unwrap();

    // Initialize logging
    logging_init::init_logging(&cli.global_args)?;

    // Invoke whatever command was requested
    cli.invoke()?;
    Ok(())
}
