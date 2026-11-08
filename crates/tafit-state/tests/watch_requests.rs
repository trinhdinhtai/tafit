//! Which Repositories are watched, and what changes made outside the app
//! re-read, driven through the synchronous reducer.

use std::path::PathBuf;

use tafit_core::{ChangeKind, FileChange, Head, RepositoryInfo, RepositoryStatus};
use tafit_state::{
    AppState, ChangeFlags, ChangeGroup, Effect, ExternalChanges, FileRef, Msg, RepoId, SelectMode,
    Watch, WatchSession, WindowId, reduce,
};

/// Opens `path` in `window` and lands its first status read, with
/// `a.txt` and `b.txt` changed in the work tree and `s.txt` staged.
fn open(state: &mut AppState, window: WindowId, path: &str) -> (RepoId, WatchSession) {
    reduce(
        state,
        Msg::OpenRepo {
            window,
            path: PathBuf::from(path),
        },
    );
    let repo_id = state.active_in(window).expect("the new tab is active").id;
    let effects = reduce(
        state,
        Msg::RepoOpened {
            repo_id,
            result: Ok(RepositoryInfo {
                name: path.to_owned(),
                head: Head::Branch("main".to_owned()),
            }),
        },
    );
    land_status(state, repo_id, &effects);
    (repo_id, session(&effects, repo_id))
}

fn changed(path: &str) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind: ChangeKind::Modified,
    }
}

/// Lands the status read among `effects` and returns the effects that asks for.
fn land_status(state: &mut AppState, repo_id: RepoId, effects: &[Effect]) -> Vec<Effect> {
    let request = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::LoadStatus { request, .. } => Some(*request),
            _ => None,
        })
        .expect("a status read");
    reduce(
        state,
        Msg::StatusLoaded {
            repo_id,
            request,
            result: Ok(RepositoryStatus {
                staged: vec![changed("s.txt")],
                unstaged: vec![changed("a.txt"), changed("b.txt")],
                ..RepositoryStatus::default()
            }),
        },
    )
}

fn session(effects: &[Effect], repo: RepoId) -> WatchSession {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Watch {
                repo_id, session, ..
            } if *repo_id == repo => Some(*session),
            _ => None,
        })
        .expect("a watch of the Repository")
}

/// What the effects ask for, by kind, in order.
fn kinds(effects: &[Effect]) -> Vec<&'static str> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::LoadStatus { .. } => "status",
            Effect::LoadInfo { .. } => "info",
            Effect::LoadDiff { .. } => "diff",
            Effect::Watch { .. } => "watch",
            Effect::Unwatch { .. } => "unwatch",
            Effect::CloseRepo { .. } => "close",
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

fn files_changed(
    state: &mut AppState,
    repo_id: RepoId,
    session: WatchSession,
    flags: ChangeFlags,
    files: Option<&[&str]>,
) -> Vec<Effect> {
    reduce(
        state,
        Msg::FilesChanged {
            repo_id,
            session,
            changes: ExternalChanges {
                flags,
                files: files.map(|files| files.iter().map(|&file| file.to_owned()).collect()),
            },
        },
    )
}

/// One open Repository, its watch, and the session the watch reports with.
fn one_watched() -> (AppState, RepoId, WatchSession) {
    let mut state = AppState::default();
    reduce(
        &mut state,
        Msg::OpenRepo {
            window: WindowId::FIRST,
            path: PathBuf::from("/work/repo"),
        },
    );
    let repo_id = state.active_repo().expect("the new tab is active");
    let effects = reduce(
        &mut state,
        Msg::RepoOpened {
            repo_id,
            result: Ok(RepositoryInfo {
                name: "repo".to_owned(),
                head: Head::Branch("main".to_owned()),
            }),
        },
    );
    assert_eq!(kinds(&effects), ["status", "watch"]);
    let session = session(&effects, repo_id);
    land_status(&mut state, repo_id, &effects);
    (state, repo_id, session)
}

fn select(state: &mut AppState, repo_id: RepoId, group: ChangeGroup, path: &str) {
    let effects = reduce(
        state,
        Msg::Select {
            repo_id,
            file: FileRef {
                group,
                path: path.to_owned(),
            },
            mode: SelectMode::Only,
        },
    );
    assert_eq!(kinds(&effects), ["diff"]);
}

#[test]
fn an_opened_active_repository_is_watched_until_its_tab_is_left() {
    let (mut state, first, _) = one_watched();
    assert_eq!(state.repo(first).unwrap().watch, Watch::Starting);

    reduce(
        &mut state,
        Msg::OpenRepo {
            window: WindowId::FIRST,
            path: PathBuf::from("/work/other"),
        },
    );
    // The new tab is active but not open yet: nothing watches it, and the
    // tab left stops being watched.
    assert_eq!(state.repo(first).unwrap().watch, Watch::Off);
    let second = state.active_repo().unwrap();
    assert_eq!(state.repo(second).unwrap().watch, Watch::Off);

    let effects = reduce(
        &mut state,
        Msg::RepoOpened {
            repo_id: second,
            result: Ok(RepositoryInfo {
                name: "other".to_owned(),
                head: Head::Branch("main".to_owned()),
            }),
        },
    );
    assert_eq!(kinds(&effects), ["status", "watch"]);
}

#[test]
fn switching_tabs_moves_the_watch_and_rereads_the_tab_returned_to() {
    let mut state = AppState::default();
    let (first, _) = open(&mut state, WindowId::FIRST, "/work/first");
    let (second, _) = open(&mut state, WindowId::FIRST, "/work/second");

    let effects = reduce(&mut state, Msg::ActivateRepo(first));

    assert!(matches!(effects[0], Effect::Unwatch { repo_id } if repo_id == second));
    assert_eq!(kinds(&effects), ["unwatch", "watch", "status", "info"]);
    assert_eq!(state.repo(first).unwrap().watch, Watch::Starting);
    assert_eq!(state.repo(second).unwrap().watch, Watch::Off);
}

#[test]
fn each_windows_active_repository_is_watched() {
    let mut state = AppState::default();
    let (first, _) = open(&mut state, WindowId::FIRST, "/work/first");
    reduce(&mut state, Msg::OpenWindow);
    let window = state.focused_window.unwrap();
    let (second, _) = open(&mut state, window, "/work/second");

    assert_eq!(state.repo(first).unwrap().watch, Watch::Starting);
    assert_eq!(state.repo(second).unwrap().watch, Watch::Starting);

    let effects = reduce(&mut state, Msg::CloseWindow(window));
    assert_eq!(kinds(&effects), ["close", "unwatch"]);
}

#[test]
fn closing_the_watched_repository_stops_its_watch() {
    let (mut state, repo_id, _) = one_watched();

    let effects = reduce(&mut state, Msg::CloseRepo(repo_id));

    assert_eq!(kinds(&effects), ["close", "unwatch"]);
}

#[test]
fn only_what_the_changes_touched_is_reread() {
    let (mut state, repo_id, session) = one_watched();

    let worktree = files_changed(
        &mut state,
        repo_id,
        session,
        ChangeFlags::WORKTREE,
        Some(&["a.txt"]),
    );
    assert_eq!(kinds(&worktree), ["status"]);

    let index = files_changed(&mut state, repo_id, session, ChangeFlags::INDEX, None);
    assert_eq!(kinds(&index), ["status"]);

    let head = files_changed(&mut state, repo_id, session, ChangeFlags::GIT_STATE, None);
    assert_eq!(kinds(&head), ["status", "info"]);

    let tag = files_changed(&mut state, repo_id, session, ChangeFlags::TAG, None);
    assert_eq!(kinds(&tag), Vec::<&str>::new());
}

#[test]
fn what_an_older_or_stopped_watch_reports_is_dropped() {
    let mut state = AppState::default();
    let (first, first_session) = open(&mut state, WindowId::FIRST, "/work/first");
    let (second, second_session) = open(&mut state, WindowId::FIRST, "/work/second");
    // Leaving the second tab and returning starts a newer watch of it.
    reduce(&mut state, Msg::ActivateRepo(first));
    let effects = reduce(&mut state, Msg::ActivateRepo(second));
    let newer = session(&effects, second);
    assert_ne!(newer, second_session);

    // The first tab isn't watched any more.
    let effects = files_changed(&mut state, first, first_session, ChangeFlags::ALL, None);
    assert!(effects.is_empty(), "{effects:?}");

    // The second tab's first watch reports too late.
    let effects = files_changed(&mut state, second, second_session, ChangeFlags::ALL, None);
    assert!(effects.is_empty(), "{effects:?}");
    reduce(
        &mut state,
        Msg::WatchChanged {
            repo_id: second,
            session: second_session,
            watch: Watch::Degraded("old".to_owned()),
        },
    );
    assert_eq!(state.repo(second).unwrap().watch, Watch::Starting);

    reduce(
        &mut state,
        Msg::WatchChanged {
            repo_id: second,
            session: newer,
            watch: Watch::Watching,
        },
    );
    assert_eq!(state.repo(second).unwrap().watch, Watch::Watching);
}

#[test]
fn the_shown_diff_is_reread_only_when_its_file_may_have_changed() {
    let (mut state, repo_id, session) = one_watched();
    select(&mut state, repo_id, ChangeGroup::Unstaged, "a.txt");

    let other = files_changed(
        &mut state,
        repo_id,
        session,
        ChangeFlags::WORKTREE,
        Some(&["b.txt"]),
    );
    assert_eq!(
        kinds(&land_status(&mut state, repo_id, &other)),
        Vec::<&str>::new()
    );

    let shown = files_changed(
        &mut state,
        repo_id,
        session,
        ChangeFlags::WORKTREE,
        Some(&["a.txt"]),
    );
    assert_eq!(kinds(&land_status(&mut state, repo_id, &shown)), ["diff"]);

    let unknown = files_changed(&mut state, repo_id, session, ChangeFlags::WORKTREE, None);
    assert_eq!(kinds(&land_status(&mut state, repo_id, &unknown)), ["diff"]);

    let index = files_changed(&mut state, repo_id, session, ChangeFlags::INDEX, Some(&[]));
    assert_eq!(kinds(&land_status(&mut state, repo_id, &index)), ["diff"]);
}

#[test]
fn a_staged_diff_is_not_reread_for_work_tree_changes() {
    let (mut state, repo_id, session) = one_watched();
    select(&mut state, repo_id, ChangeGroup::Staged, "s.txt");

    let worktree = files_changed(&mut state, repo_id, session, ChangeFlags::WORKTREE, None);
    assert_eq!(
        kinds(&land_status(&mut state, repo_id, &worktree)),
        Vec::<&str>::new()
    );

    let index = files_changed(&mut state, repo_id, session, ChangeFlags::INDEX, None);
    assert_eq!(kinds(&land_status(&mut state, repo_id, &index)), ["diff"]);
}

#[test]
fn a_reread_asked_for_by_a_superseded_status_read_still_happens() {
    let (mut state, repo_id, session) = one_watched();
    select(&mut state, repo_id, ChangeGroup::Unstaged, "a.txt");

    files_changed(
        &mut state,
        repo_id,
        session,
        ChangeFlags::WORKTREE,
        Some(&["a.txt"]),
    );
    let later = files_changed(
        &mut state,
        repo_id,
        session,
        ChangeFlags::WORKTREE,
        Some(&["b.txt"]),
    );

    assert_eq!(kinds(&land_status(&mut state, repo_id, &later)), ["diff"]);
}

#[test]
fn changes_flags_combine() {
    let flags = ChangeFlags::WORKTREE | ChangeFlags::TAG;
    assert!(flags.contains(ChangeFlags::TAG));
    assert!(!flags.contains(ChangeFlags::WORKTREE | ChangeFlags::INDEX));
    assert!(flags.intersects(ChangeFlags::WORKTREE | ChangeFlags::INDEX));
    assert!(ChangeFlags::default().is_empty());
    assert_eq!(format!("{flags:?}"), "ChangeFlags(WORKTREE | TAG)");
    assert!(ChangeFlags::ALL.contains(ChangeFlags::CONFIG));
}
