//! What the backend tells the watcher: where a Repository's files are and
//! which of them git ignores.

mod support;

use support::TestRepo;
use tafit_core::{CancellationToken, GitBackend};
use tafit_git_gix::GixBackend;

fn ignored(repo: &TestRepo, paths: &[&str]) -> Vec<bool> {
    let cancel = CancellationToken::default();
    let paths: Vec<String> = paths.iter().map(|&path| path.to_owned()).collect();
    GixBackend::default()
        .open(repo.path(), &cancel)
        .expect("open")
        .ignored(&paths, &cancel)
        .expect("read ignore rules")
}

#[test]
fn gitignore_rules_cover_files_and_whatever_is_under_an_ignored_folder() {
    let repo = TestRepo::with_commit("ignore");
    repo.commit_files(&[(".gitignore", "target/\n*.log\n!keep.log\n")], "ignore");
    repo.write("sub/.gitignore", "local.txt\n");

    assert_eq!(
        ignored(
            &repo,
            &[
                "target/",
                "target/debug/out.o",
                "build.log",
                "keep.log",
                "src/main.rs",
                "sub/local.txt",
                "local.txt",
            ],
        ),
        [true, true, true, false, false, true, false]
    );
}

#[test]
fn a_tracked_file_matching_a_rule_is_not_ignored() {
    let repo = TestRepo::with_commit("tracked");
    repo.commit_files(&[("debug.log", "kept\n")], "track a log");
    repo.write(".gitignore", "*.log\n");

    assert_eq!(ignored(&repo, &["debug.log", "other.log"]), [false, true]);
}

#[test]
fn info_exclude_is_an_ignore_rule() {
    let repo = TestRepo::with_commit("exclude");
    repo.write(".git/info/exclude", "secret.txt\n");

    assert_eq!(ignored(&repo, &["secret.txt"]), [true]);
}

#[test]
fn paths_name_the_work_tree_and_git_dir() {
    let repo = TestRepo::with_commit("paths");
    let cancel = CancellationToken::default();
    let paths = GixBackend::default()
        .open(repo.path(), &cancel)
        .expect("open")
        .paths();

    let workdir = paths.workdir.expect("a work tree");
    assert_eq!(
        std::fs::canonicalize(&workdir).unwrap(),
        std::fs::canonicalize(repo.path()).unwrap()
    );
    assert_eq!(
        std::fs::canonicalize(&paths.git_dir).unwrap(),
        std::fs::canonicalize(workdir.join(".git")).unwrap()
    );
    assert_eq!(paths.common_dir, paths.git_dir);
}

#[test]
fn a_linked_worktree_shares_the_common_dir() {
    let repo = TestRepo::with_commit("main-tree");
    let linked = repo.path().with_file_name("linked-tree");
    repo.git(&["worktree", "add", "--quiet", linked.to_str().unwrap()]);
    let cancel = CancellationToken::default();
    let paths = GixBackend::default()
        .open(&linked, &cancel)
        .expect("open")
        .paths();

    assert_ne!(paths.git_dir, paths.common_dir);
    assert_eq!(
        std::fs::canonicalize(&paths.common_dir).unwrap(),
        std::fs::canonicalize(repo.path().join(".git")).unwrap()
    );
}
