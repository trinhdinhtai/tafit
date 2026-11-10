//! The commit history of a Repository: every commit reachable from HEAD and
//! its branches, remote branches and tags, newest first, with the refs that
//! point at each.

mod support;

use std::path::Path;

use support::{TestRepo, TestStore};
use tafit_core::{RefKind, RefLabel};
use tafit_state::{AppState, HistoryState, Loadable, Msg, RepoId, WindowId};

/// Opens `path` and waits until its history index has loaded.
fn open(store: &TestStore, path: &Path) -> RepoId {
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: path.to_owned(),
    });
    store.flush();
    let state = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| matches!(repo.history.index, Loadable::Ready(_)))
    });
    state
        .active_repo()
        .expect("the opened Repository is active")
}

fn history(state: &AppState, repo_id: RepoId) -> &HistoryState {
    &state.repo(repo_id).expect("the Repository is open").history
}

/// The history's commits, newest first, as full hex ids.
fn ids(state: &AppState, repo_id: RepoId) -> Vec<String> {
    let history = history(state, repo_id);
    (0..history.len())
        .map(|row| {
            history
                .commit_id(row)
                .expect("a row of the index")
                .to_string()
        })
        .collect()
}

/// `git rev-list --date-order` of what the history starts from.
fn git_order(repo: &TestRepo) -> Vec<String> {
    repo.git(&[
        "rev-list",
        "--date-order",
        "HEAD",
        "--branches",
        "--remotes",
        "--tags",
    ])
    .lines()
    .map(str::to_owned)
    .collect()
}

/// The row of the commit `rev` names.
fn row_of(state: &AppState, repo_id: RepoId, repo: &TestRepo, rev: &str) -> usize {
    let id = repo.git(&["rev-parse", &format!("{rev}^{{commit}}")]);
    ids(state, repo_id)
        .iter()
        .position(|row| *row == id)
        .unwrap_or_else(|| panic!("{rev} is not in the history"))
}

fn label(kind: RefKind, name: &str, head: bool) -> RefLabel {
    RefLabel {
        kind,
        name: name.to_owned(),
        head,
    }
}

#[test]
fn the_history_lists_commits_newest_first_as_git_does() {
    let repo = TestRepo::init("order");
    repo.commit_at("first", 1_000);
    repo.commit_at("second", 2_000);
    repo.git(&["switch", "--quiet", "-c", "feature"]);
    repo.commit_at("on feature", 3_000);
    repo.git(&["switch", "--quiet", "main"]);
    repo.commit_at("third", 4_000);
    repo.git(&["tag", "v1", "HEAD~1"]);
    // A merge whose second parent is older than the main line it joins.
    repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", "feature"]);
    repo.commit_at("after merge", 6_000);
    let store = TestStore::new();

    let repo_id = open(&store, repo.path());

    let state = store.store.snapshot();
    assert_eq!(ids(&state, repo_id), git_order(&repo));
    assert_eq!(history(&state, repo_id).len(), 6);
}

#[test]
fn every_branch_remote_branch_and_tag_starts_the_history() {
    let repo = TestRepo::init("tips");
    repo.commit_at("base", 1_000);
    for (branch, time) in [("side", 2_000), ("old-tag", 3_000), ("remote", 4_000)] {
        repo.git(&["switch", "--quiet", "-c", branch, "main"]);
        repo.commit_at(&format!("only on {branch}"), time);
    }
    repo.git(&["tag", "-a", "-m", "annotated", "v1", "old-tag"]);
    repo.git(&["update-ref", "refs/remotes/origin/remote", "remote"]);
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["branch", "--quiet", "-D", "old-tag", "remote"]);
    let store = TestStore::new();

    let repo_id = open(&store, repo.path());

    let state = store.store.snapshot();
    assert_eq!(ids(&state, repo_id), git_order(&repo));
    assert_eq!(history(&state, repo_id).len(), 4);
}

#[test]
fn a_hidden_ref_never_starts_the_history() {
    let repo = TestRepo::init("hidden");
    repo.commit_at("shown", 1_000);
    repo.git(&["switch", "--quiet", "-c", "tool-state"]);
    repo.commit_at("only a tool's ref reaches this", 2_000);
    repo.git(&["update-ref", "refs/tool/state", "tool-state"]);
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["branch", "--quiet", "-D", "tool-state"]);
    repo.git(&["notes", "add", "-m", "a note", "HEAD"]);
    repo.write("stashed.txt", "stashed\n");
    repo.git(&["stash", "--quiet", "--include-untracked"]);
    let store = TestStore::new();

    let repo_id = open(&store, repo.path());

    let state = store.store.snapshot();
    assert_eq!(ids(&state, repo_id), [repo.git(&["rev-parse", "HEAD"])]);
}

#[test]
fn a_detached_head_is_in_the_history_even_on_no_branch() {
    let repo = TestRepo::init("detached");
    repo.commit_at("on main", 1_000);
    repo.git(&["switch", "--quiet", "--detach"]);
    repo.commit_at("only HEAD reaches this", 2_000);
    let store = TestStore::new();

    let repo_id = open(&store, repo.path());

    let state = store.store.snapshot();
    assert_eq!(ids(&state, repo_id), git_order(&repo));
    let head = row_of(&state, repo_id, &repo, "HEAD");
    assert_eq!(head, 0);
    assert_eq!(
        history(&state, repo_id).labels(head),
        [label(RefKind::Head, "HEAD", true)]
    );
    let main = row_of(&state, repo_id, &repo, "main");
    assert_eq!(
        history(&state, repo_id).labels(main),
        [label(RefKind::Branch, "main", false)]
    );
}

#[test]
fn each_commit_shows_the_branches_remote_branches_and_tags_on_it() {
    let repo = TestRepo::init("labels");
    repo.commit_at("first", 1_000);
    repo.git(&["tag", "-a", "-m", "annotated", "v1"]);
    repo.git(&["tag", "light"]);
    repo.commit_at("second", 2_000);
    repo.git(&["branch", "feature"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let store = TestStore::new();

    let repo_id = open(&store, repo.path());

    let state = store.store.snapshot();
    let history = history(&state, repo_id);
    // HEAD's branch first, then branches, remote branches and tags, by name.
    // A remote's own HEAD only says which branch it checks out.
    assert_eq!(
        history.labels(row_of(&state, repo_id, &repo, "main")),
        [
            label(RefKind::Branch, "main", true),
            label(RefKind::Branch, "feature", false),
            label(RefKind::RemoteBranch, "origin/main", false),
        ]
    );
    assert_eq!(
        history.labels(row_of(&state, repo_id, &repo, "v1")),
        [
            label(RefKind::Tag, "light", false),
            label(RefKind::Tag, "v1", false),
        ]
    );
}

#[test]
fn a_repository_with_no_commits_has_an_empty_history() {
    let repo = TestRepo::init("empty");
    let store = TestStore::new();

    let repo_id = open(&store, repo.path());

    assert_eq!(history(&store.store.snapshot(), repo_id).len(), 0);
}

#[test]
fn a_bare_repository_has_a_history_too() {
    let source = TestRepo::init("source");
    source.commit_at("first", 1_000);
    source.commit_at("second", 2_000);
    let bare = TestRepo::init_bare("bare.git");
    source.git(&[
        "push",
        "--quiet",
        &bare.path().display().to_string(),
        "main",
    ]);
    let store = TestStore::new();

    let repo_id = open(&store, bare.path());

    let state = store.store.snapshot();
    assert_eq!(ids(&state, repo_id), git_order(&bare));
}
