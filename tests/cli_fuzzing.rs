//! CLI fuzzing tests using figue's arbitrary helper assertions.

use locate_git_projects_on_my_computer::cli::Cli;

#[test]
fn fuzz_cli_args_consistency() {
    figue::assert_to_args_consistency::<Cli>(5000)
        .expect("figue helper consistency check should pass");
}

#[test]
fn fuzz_cli_args_roundtrip() {
    figue::assert_to_args_roundtrip::<Cli>(500).expect("figue helper roundtrip check should pass");
}
