//! GPUI smoke tests: a Window renders a Repository Tab, the git notices and
//! the settings page. Logic is tested through the store (tafit-state), not here.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{TestAppContext, VisualTestContext};
use tafit_core::{
    ChangeKind, FileChange, GitEnvironment, GitError, GitVersion, Head, RepositoryInfo,
    RepositoryStatus,
};
use tafit_git_gix::GixBackend;
use tafit_state::{AppState, AppStore, Effect, Msg, reduce};
use tafit_ui_gpui::TafitView;

/// A snapshot with one opened Repository, built with the synchronous reducer.
fn state_with_open_repository() -> AppState {
    let mut state = AppState::default();
    reduce(&mut state, Msg::OpenRepo(PathBuf::from("/work/my-repo")));
    let repo_id = state.active_repo.expect("OpenRepo activates the new tab");
    reduce(
        &mut state,
        Msg::RepoOpened {
            repo_id,
            result: Ok(RepositoryInfo {
                name: "my-repo".to_owned(),
                head: Head::Branch("main".to_owned()),
            }),
        },
    );
    state
}

/// `state` after git detection reported `result`.
fn with_git(mut state: AppState, result: Result<GitEnvironment, GitError>) -> AppState {
    let [Effect::DetectGit { request }] = reduce(&mut state, Msg::DetectGit)[..] else {
        panic!("DetectGit asks for one detection");
    };
    reduce(&mut state, Msg::GitDetected { request, result });
    state
}

fn git_environment(version_output: &str) -> GitEnvironment {
    GitEnvironment {
        git_path: PathBuf::from("/usr/bin/git"),
        version: GitVersion::parse(version_output).expect("a valid version"),
        gpg_path: Some(PathBuf::from("/usr/bin/gpg")),
        ssh_keygen_path: None,
    }
}

/// Opens a Window showing `state`.
fn render(state: AppState, cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (store, _events) = AppStore::with_state(Arc::new(GixBackend::default()), state);
    // No events receiver: the live poller is off under GPUI's test scheduler.
    let (_view, cx) = cx.add_window_view(|_window, cx| TafitView::new(Arc::new(store), None, cx));
    cx.run_until_parked();
    cx
}

#[gpui::test]
fn window_renders_the_repository_tab(cx: &mut TestAppContext) {
    let cx = render(state_with_open_repository(), cx);

    assert!(cx.debug_bounds("repository-tab").is_some(), "tab bar entry");
    assert!(
        cx.debug_bounds("repository-name").is_some(),
        "Repository name"
    );
    assert!(cx.debug_bounds("repository-head").is_some(), "HEAD line");
    assert!(
        cx.debug_bounds("settings-button").is_some(),
        "Settings button"
    );
}

#[gpui::test]
fn an_old_git_shows_a_notice_above_the_repository_tab(cx: &mut TestAppContext) {
    let state = with_git(
        state_with_open_repository(),
        Ok(git_environment("git version 2.39.3 (Apple Git-146)")),
    );

    let cx = render(state, cx);

    assert!(cx.debug_bounds("git-outdated-notice").is_some(), "notice");
    assert!(
        cx.debug_bounds("repository-name").is_some(),
        "the Repository Tab stays usable"
    );
}

#[gpui::test]
fn a_missing_git_shows_install_instructions(cx: &mut TestAppContext) {
    let state = with_git(
        state_with_open_repository(),
        Err(GitError::GitNotFound("no `git` on PATH".to_owned())),
    );

    let cx = render(state, cx);

    assert!(cx.debug_bounds("git-missing-notice").is_some(), "notice");
    assert!(
        cx.debug_bounds("git-error-notice").is_none(),
        "not an error"
    );
}

#[gpui::test]
fn a_git_that_fails_shows_its_error_not_install_instructions(cx: &mut TestAppContext) {
    let state = with_git(
        state_with_open_repository(),
        Err(GitError::CommandFailed {
            command: "git --version".to_owned(),
            exit_code: Some(3),
            stdout: String::new(),
            stderr: "fatal: broken install".to_owned(),
        }),
    );

    let cx = render(state, cx);

    assert!(cx.debug_bounds("git-error-notice").is_some(), "notice");
    assert!(
        cx.debug_bounds("git-missing-notice").is_none(),
        "git is installed"
    );
}

#[gpui::test]
fn the_settings_page_shows_the_programs_found(cx: &mut TestAppContext) {
    let mut state = with_git(
        state_with_open_repository(),
        Ok(git_environment("git version 2.53.0.windows.2")),
    );
    reduce(&mut state, Msg::OpenSettings);

    let cx = render(state, cx);

    assert!(cx.debug_bounds("settings-page").is_some(), "settings page");
    for row in [
        "settings-git-path",
        "settings-git-version",
        "settings-gpg-path",
        "settings-ssh-keygen-path",
    ] {
        assert!(cx.debug_bounds(row).is_some(), "{row}");
    }
    assert!(
        cx.debug_bounds("git-outdated-notice").is_none(),
        "2.53 is supported"
    );
}

/// `state`'s Active Repository after its status read reported `status`.
fn with_status(mut state: AppState, status: RepositoryStatus) -> AppState {
    let repo_id = state.active_repo.expect("a Repository is open");
    let [Effect::LoadStatus { request, .. }] = reduce(&mut state, Msg::RefreshStatus(repo_id))[..]
    else {
        panic!("RefreshStatus asks for one status read");
    };
    reduce(
        &mut state,
        Msg::StatusLoaded {
            repo_id,
            request,
            result: Ok(status),
        },
    );
    state
}

fn change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind,
    }
}

#[gpui::test]
fn the_change_list_shows_its_three_groups(cx: &mut TestAppContext) {
    let status = RepositoryStatus {
        staged: vec![change(
            "src/new.rs",
            ChangeKind::Renamed {
                from: "src/old.rs".to_owned(),
            },
        )],
        unstaged: vec![
            change("README.md", ChangeKind::Modified),
            change("gone.txt", ChangeKind::Deleted),
        ],
        untracked: vec![change("notes/", ChangeKind::Added)],
        ..RepositoryStatus::default()
    };

    let cx = render(with_status(state_with_open_repository(), status), cx);

    assert!(cx.debug_bounds("status-panel").is_some(), "change list");
    assert!(
        cx.debug_bounds("status-refresh").is_some(),
        "refresh button"
    );
    for group in ["status-staged", "status-unstaged", "status-untracked"] {
        assert!(cx.debug_bounds(group).is_some(), "{group} header");
    }
    assert!(cx.debug_bounds("status-row").is_some(), "file rows");
}

#[gpui::test]
fn a_clean_repository_says_there_are_no_changes(cx: &mut TestAppContext) {
    let state = with_status(state_with_open_repository(), RepositoryStatus::default());

    let cx = render(state, cx);

    assert!(
        cx.debug_bounds("status-clean").is_some(),
        "no-changes message"
    );
    assert!(cx.debug_bounds("status-row").is_none(), "no file rows");
}
