mod support;

use std::thread;
use std::time::Duration;

use support::{FakeGit, TestDir, TestStore};
use tafit_core::{GitEnvironment, GitError, GitVersion};
use tafit_state::{AppState, Effect, Loadable, Msg, reduce};

/// Detects git in `store` and returns the settled result.
fn detect(store: &TestStore) -> Loadable<GitEnvironment, GitError> {
    store.dispatch(Msg::DetectGit);
    let state = store.wait_until(|state: &AppState| !matches!(state.git, Loadable::Loading));
    state.git.clone()
}

#[test]
fn detecting_the_system_git_reports_its_path_and_version() {
    let store = TestStore::new();

    let Loadable::Ready(git) = detect(&store) else {
        panic!("the git that runs these tests must be detected");
    };

    assert!(git.git_path.is_absolute() && git.git_path.is_file());
    assert!(git.version.major >= 2, "version: {:?}", git.version);
}

#[test]
fn a_git_older_than_2_53_is_detected_but_marked_unsupported() {
    let fake = FakeGit::printing("git version 2.39.3 (Apple Git-146)", "", 0);
    let store = TestStore::with_git(fake.program());

    let Loadable::Ready(git) = detect(&store) else {
        panic!("an old git must still be detected");
    };

    assert_eq!(git.version.text, "2.39.3");
    assert!(!git.version.is_supported());
    assert_eq!(git.git_path, fake.program());
}

#[test]
fn a_missing_git_is_reported_as_not_found() {
    let dir = TestDir::new("no-git-here");
    let store = TestStore::with_git(&dir.path.join("git"));

    let Loadable::Error(error) = detect(&store) else {
        panic!("a missing git must not be detected");
    };

    assert!(
        matches!(error, GitError::GitNotFound(_)),
        "a missing git must be told apart from a broken one: {error:?}"
    );
    assert!(
        error.to_string().starts_with("git was not found"),
        "unclear message: {error}"
    );
}

#[test]
fn a_failing_git_reports_its_exit_code_and_everything_it_printed() {
    let fake = FakeGit::printing("partial output", "fatal: broken install", 3);
    let store = TestStore::with_git(fake.program());

    let Loadable::Error(error) = detect(&store) else {
        panic!("a failing git must not be detected");
    };

    let GitError::CommandFailed {
        command,
        exit_code,
        stdout,
        stderr,
    } = &error
    else {
        panic!("expected a failed command, got {error:?}");
    };
    assert_eq!(command, "git --version");
    assert_eq!(*exit_code, Some(3));
    assert_eq!(stdout.trim(), "partial output");
    assert_eq!(stderr.trim(), "fatal: broken install");
    let message = error.to_string();
    assert!(message.contains("`git --version`"), "message: {message}");
    assert!(message.contains("exit code 3"), "message: {message}");
    assert!(
        message.contains("fatal: broken install"),
        "message: {message}"
    );
    assert!(message.contains("partial output"), "message: {message}");
}

#[test]
fn signing_tools_come_from_the_user_config_but_not_a_repository_config() {
    let tools = TestDir::new("tools");
    let gpg = tools.path.join("my-gpg");
    let repo_ssh_keygen = tools.path.join("repo-ssh-keygen");
    std::fs::write(&gpg, "").expect("write fake gpg");
    std::fs::write(&repo_ssh_keygen, "").expect("write fake ssh-keygen");
    let fake = FakeGit::with_config(&[
        format!("global\tgpg.program {}", gpg.display()),
        format!("local\tgpg.ssh.program {}", repo_ssh_keygen.display()),
    ]);
    let store = TestStore::with_git(fake.program());

    let Loadable::Ready(git) = detect(&store) else {
        panic!("git must be detected");
    };

    assert_eq!(git.gpg_path, Some(gpg));
    assert_ne!(git.ssh_keygen_path, Some(repo_ssh_keygen));
}

#[test]
fn an_older_detection_finishing_late_does_not_replace_a_newer_one() {
    let mut state = AppState::default();
    let [Effect::DetectGit { request: first }] = reduce(&mut state, Msg::DetectGit)[..] else {
        panic!("DetectGit asks for one detection");
    };
    let [Effect::DetectGit { request: second }] = reduce(&mut state, Msg::DetectGit)[..] else {
        panic!("DetectGit asks for one detection");
    };
    let environment = |version: &str| GitEnvironment {
        git_path: "/usr/bin/git".into(),
        version: GitVersion::parse(version).expect("a valid version"),
        gpg_path: None,
        ssh_keygen_path: None,
    };

    reduce(
        &mut state,
        Msg::GitDetected {
            request: second,
            result: Ok(environment("git version 2.53.0")),
        },
    );
    reduce(
        &mut state,
        Msg::GitDetected {
            request: first,
            result: Ok(environment("git version 2.39.3")),
        },
    );

    let Loadable::Ready(git) = &state.git else {
        panic!("the newer detection's result must stay");
    };
    assert_eq!(git.version.text, "2.53.0");
}

#[test]
fn shutting_down_the_store_stops_a_running_git_and_everything_it_started() {
    let fake = FakeGit::hanging();
    let store = TestStore::with_git(fake.program());
    store.dispatch(Msg::DetectGit);
    fake.wait_until_started();

    // Shutdown cancels every effect still running.
    drop(store);

    // Left alone, the fake marks `finished` about two seconds after starting.
    thread::sleep(Duration::from_secs(4));
    assert!(
        !fake.finished(),
        "git or a process it started kept running after cancellation"
    );
}
