//! CLI fuzzing tests using figue's arbitrary helper assertions.

use figue::TestToArgsConsistencyConfig;
use figue::TestToArgsRoundTrip;
use locate_git_projects_on_my_computer::cli::Cli;

#[test]
fn fuzz_cli_args_consistency() {
    figue::assert_to_args_consistency::<Cli>(TestToArgsConsistencyConfig {
        success_count: 5000,
        ..Default::default()
    })
    .expect("figue helper consistency check should pass");
}

#[test]
fn fuzz_cli_args_roundtrip() {
    figue::assert_to_args_roundtrip::<Cli>(TestToArgsRoundTrip {
        success_count_global: 500,
        max_attempts_global: 500 * 80,
        ..Default::default()
    })
    .expect("figue helper roundtrip check should pass");
}
