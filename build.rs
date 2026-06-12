use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    add_exe_resources();
    add_git_revision();
    add_build_unix_ms();
}

/// Embeds Windows resources (like application icon) into the executable.
fn add_exe_resources() {
    println!("cargo:rerun-if-changed=resources");

    embed_resource::compile("resources/app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed resources");
}

/// In your code you can now access git revision using
/// ```rust
/// let git_rev = option_env!("GIT_REVISION").unwrap_or("unknown");
/// ```
fn add_git_revision() {
    if let Some(head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }

    if let Some(head_ref) = git_output(&["symbolic-ref", "--quiet", "HEAD"]) {
        if let Some(head_ref_path) = git_output(&["rev-parse", "--git-path", &head_ref]) {
            println!("cargo:rerun-if-changed={head_ref_path}");
        }
    }

    // Try to get a short git revision; on failure, set to "unknown".
    let rev =
        git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_REVISION={rev}");
}

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .and_then(|o| o.status.success().then_some(o.stdout))
        .and_then(|v| String::from_utf8(v).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn add_build_unix_ms() {
    println!("cargo:rerun-if-env-changed=LOCATE_GIT_PROJECTS_BUILD_UNIX_MS");
    let build_unix_ms = std::env::var("LOCATE_GIT_PROJECTS_BUILD_UNIX_MS").unwrap_or_else(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_millis()
            .to_string()
    });

    println!("cargo:rustc-env=BUILD_UNIX_MS={build_unix_ms}");
}
