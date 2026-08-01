use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=resources");

    embed_resource::compile("resources/app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed resources");

    for (name, args) in [
        ("GIT_REVISION", vec!["rev-parse", "--short", "HEAD"]),
        ("GIT_BRANCH", vec!["branch", "--show-current"]),
        ("GIT_REPOSITORY_URL", vec!["remote", "get-url", "origin"]),
    ] {
        let value = git_output(&args).unwrap_or_else(|| String::from("unknown"));
        println!("cargo:rustc-env={name}={value}");
    }

    let dirty = git_output_allow_empty(&["status", "--short", "--untracked-files=no"]).map_or_else(
        || String::from("unknown"),
        |status| {
            if status.is_empty() {
                "clean".to_owned()
            } else {
                "dirty".to_owned()
            }
        },
    );
    println!("cargo:rustc-env=GIT_WORKTREE_STATUS={dirty}");

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after Unix epoch")
        .as_secs();
    println!("cargo:rustc-env=BUILD_TIMESTAMP_UNIX={timestamp}");
}

fn git_output(args: &[&str]) -> Option<String> {
    git_output_allow_empty(args).filter(|value| !value.is_empty())
}

fn git_output_allow_empty(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output.stdout))
        .and_then(|value| String::from_utf8(value).ok())
        .map(|value| value.trim().to_owned())
}
