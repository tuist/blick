use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use blick::cli::ReviewArgs;
use blick::commands::review;
use blick::config::{AgentKind, ConfigFile};
use blick::git::collect_diff_in;
use blick::run_record::{read_manifest, resolve_run_dir};
use tempfile::TempDir;

struct TestRepo {
    temp_dir: TempDir,
    root: PathBuf,
}

impl TestRepo {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("temporary directory should be created");
        let root = temp_dir.path().join("repo");
        fs::create_dir_all(&root).expect("repository directory should be created");
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.name", "Blick Tests"]);
        run_git(&root, &["config", "user.email", "blick@example.com"]);

        Self { temp_dir, root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, path: &str, contents: &str) {
        let file = self.root.join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).expect("parent directory should be created");
        }
        fs::write(file, contents).expect("file should be written");
    }

    fn commit_all(&self, message: &str) {
        run_git(&self.root, &["add", "."]);
        run_git(&self.root, &["commit", "-m", message]);
    }
}

fn run_git(repo_root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .status()
        .expect("git should run");

    assert!(
        status.success(),
        "git command failed: git {}",
        args.join(" ")
    );
}

#[test]
fn collects_tracked_and_untracked_changes_from_a_temp_repo() {
    let repo = TestRepo::new();
    repo.write("src/main.rs", "fn main() {\n    println!(\"hello\");\n}\n");
    repo.commit_all("initial commit");

    repo.write(
        "src/main.rs",
        "fn main() {\n    println!(\"hello, blick\");\n}\n",
    );
    repo.write("notes/todo.txt", "remember the shellspec suite\n");

    let diff =
        collect_diff_in(repo.path(), "HEAD", usize::MAX).expect("diff collection should succeed");

    assert!(diff.files.iter().any(|file| file == "src/main.rs"));
    assert!(diff.files.iter().any(|file| file == "notes/todo.txt"));
    assert!(diff.diff.contains("hello, blick"));
    assert!(diff.diff.contains("notes/todo.txt"));
    assert!(!diff.truncated);
}

#[test]
fn supports_repositories_without_a_head_commit() {
    let repo = TestRepo::new();
    repo.write("src/lib.rs", "pub fn answer() -> u32 { 42 }\n");

    let diff =
        collect_diff_in(repo.path(), "HEAD", usize::MAX).expect("diff collection should succeed");

    assert_eq!(diff.files, vec!["src/lib.rs".to_owned()]);
    assert!(diff.diff.contains("pub fn answer() -> u32 { 42 }"));
}

#[test]
fn returns_a_lowercase_error_outside_a_git_repo() {
    let repo = TestRepo::new();
    let outside_repo = repo.temp_dir.path().join("outside");
    fs::create_dir_all(&outside_repo).expect("outside directory should be created");

    let error = collect_diff_in(&outside_repo, "HEAD", usize::MAX)
        .expect_err("non-repository directories should fail");

    assert_eq!(
        error.to_string(),
        "blick review must run inside a git working tree"
    );
}

#[test]
fn fails_when_the_requested_base_revision_is_missing() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.commit_all("initial commit");
    repo.write("README.md", "hello, blick\n");

    let error = collect_diff_in(repo.path(), "missing-base", usize::MAX)
        .expect_err("missing revisions should fail");

    assert_eq!(error.to_string(), "git revision missing-base was not found");
}

#[test]
fn truncates_large_diffs() {
    let repo = TestRepo::new();
    repo.write("big.txt", &"abcdef\n".repeat(200));
    repo.commit_all("initial commit");
    repo.write("big.txt", &"uvwxyz\n".repeat(200));

    let diff = collect_diff_in(repo.path(), "HEAD", 64).expect("diff collection should succeed");

    assert!(diff.truncated);
    assert!(diff.diff.contains("[diff truncated by blick]"));
}

#[tokio::test]
async fn review_writes_an_empty_manifest_when_the_diff_is_empty() {
    let repo = TestRepo::new();
    let config = ConfigFile::starter(AgentKind::Codex, None)
        .to_toml()
        .expect("starter config should serialize");
    repo.write("blick.toml", &config);
    repo.commit_all("initial commit");

    review::run(ReviewArgs {
        name: None,
        agent: None,
        model: None,
        base: Some("HEAD".to_owned()),
        json: true,
        stream: false,
        max_concurrency: None,
        repo: Some(repo.path().to_path_buf()),
    })
    .await
    .expect("review should succeed when there is no diff");

    let run_dir =
        resolve_run_dir(repo.path(), None).expect("latest run should resolve after review");
    let manifest =
        read_manifest(&run_dir.join("manifest.json")).expect("empty run manifest should exist");

    assert_eq!(manifest.base, "HEAD");
    assert!(manifest.tasks.is_empty());
}
