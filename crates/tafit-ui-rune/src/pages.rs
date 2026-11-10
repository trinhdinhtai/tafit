//! The pages a Window shows in place of a Repository Tab: Open Repository,
//! Settings, and the notices about git.

use std::path::{Path, PathBuf};

use rune::{El, FileDialogOptions, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, Kit, ScrollBar, Toast, ToastRegion};
use tafit_core::{GitEnvironment, GitError, GitVersion};
use tafit_state::{AppState, Loadable, Msg, Page, WindowId};

use crate::Ui;
use crate::tabs::folder_name;

/// Where users get git.
const GIT_DOWNLOAD_URL: &str = "https://git-scm.com/downloads";

/// A page's title with its buttons on the right.
fn page_header(title: &str, k: &Kit, buttons: impl IntoIterator<Item = El>) -> El {
    row()
        .w_full()
        .gap(8.0)
        .items_center()
        .child(text(title).font(k.ty.body_medium).color(k.c.text_strong))
        .child(crate::spacer())
        .children(buttons)
}

fn page(key: &str) -> El {
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(24.0)
        .gap(12.0)
}

/// A folder picker, and the pinned and recent Repositories to open again
/// with one click. Shown in a Window with no Repository Tab, or from "+".
pub(crate) fn open_repository(state: &AppState, window: WindowId, k: &Kit) -> El {
    let has_tabs = state.tabs(window).next().is_some();
    let open_folder = Button::new("Open Folder…")
        .on_press_tx(prompt_for_repository)
        .build(k)
        .key("open-folder");
    let close = has_tabs.then(|| {
        Button::new("Close")
            .variant(ButtonVariant::Ghost)
            .on_press(|s: &mut Ui| s.show_page(Page::RepositoryTab))
            .build(k)
            .key("close-open-repository")
    });
    let rows: Vec<El> = state
        .repository_list()
        .map(|(path, pinned)| repository_row(path, pinned, window, k))
        .collect();
    let list = if rows.is_empty() {
        text("Repositories you open are listed here.")
            .font(k.ty.ui)
            .color(k.c.text_muted)
    } else {
        col()
            .key("repository-list")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(col().w_full().gap(2.0).children(rows))
            .scroll_bar(k)
    };
    page("open-repository-page")
        .child(page_header(
            "Open a Repository",
            k,
            std::iter::once(open_folder).chain(close),
        ))
        .child(
            text("Pinned and recent")
                .font(k.ty.ui)
                .color(k.c.text_muted),
        )
        .child(list)
}

fn repository_row(path: &Path, pinned: bool, window: WindowId, k: &Kit) -> El {
    let open_path = path.to_owned();
    let pin_path: PathBuf = path.to_owned();
    let pin = Button::new(if pinned { "Unpin" } else { "Pin" })
        .variant(ButtonVariant::Ghost)
        .on_press(move |s: &mut Ui| {
            s.dispatch(if pinned {
                Msg::UnpinRepo(pin_path.clone())
            } else {
                Msg::PinRepo(pin_path.clone())
            });
        })
        .build(k)
        .key(if pinned {
            "unpin-repository"
        } else {
            "pin-repository"
        });
    row()
        .key(format!("repository-list-row:{}", path.display()))
        .w_full()
        .gap(12.0)
        .px(10.0)
        .py(6.0)
        .radius(6.0)
        .items_center()
        .role("button")
        .label(folder_name(path))
        .focusable(true)
        .hover(k.c.surface_hover)
        .on_press(move |s: &mut Ui| {
            s.dispatch(Msg::OpenRepo {
                window,
                path: open_path.clone(),
            });
        })
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(text(folder_name(path)).font(k.ty.ui).color(k.c.text_base))
                .child(
                    text(path.display().to_string())
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted),
                ),
        )
        .child(pin)
}

/// Asks for a folder and opens it in this Window.
fn prompt_for_repository(_: &mut Ui, tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open Repository".into(),
        ..Default::default()
    };
    tx.open_folder(options, |s: &mut Ui, picked, _| {
        // Cancelled, or the picker couldn't open: nothing to do.
        if let Ok(Some(path)) = picked {
            s.dispatch(Msg::OpenRepo {
                window: s.window,
                path,
            });
        }
    });
}

/// The settings page: for now, the programs tafit runs.
pub(crate) fn settings(state: &AppState, k: &Kit) -> El {
    let programs = ProgramLabels::new(&state.git);
    let rows = [
        ("Git", programs.git, "settings-git-path"),
        ("Git version", programs.version, "settings-git-version"),
        ("GPG", programs.gpg, "settings-gpg-path"),
        (
            "ssh-keygen",
            programs.ssh_keygen,
            "settings-ssh-keygen-path",
        ),
    ];
    let close = Button::new("Close")
        .variant(ButtonVariant::Ghost)
        .on_press(|s: &mut Ui| s.show_page(Page::RepositoryTab))
        .build(k)
        .key("close-settings");
    page("settings-page")
        .child(page_header("Settings", k, [close]))
        .child(text("Programs").font(k.ty.ui).color(k.c.text_muted))
        .children(rows.into_iter().map(|(label, value, key)| {
            row()
                .gap(12.0)
                .child(text(label).w(120.0).font(k.ty.ui).color(k.c.text_muted))
                .child(text(value).key(key).font(k.ty.mono).color(k.c.text_base))
        }))
}

/// What the settings page shows for each program tafit runs.
struct ProgramLabels {
    git: String,
    version: String,
    gpg: String,
    ssh_keygen: String,
}

impl ProgramLabels {
    fn new(git: &Loadable<GitEnvironment, GitError>) -> Self {
        let same = |label: &str| Self {
            git: label.to_owned(),
            version: label.to_owned(),
            gpg: label.to_owned(),
            ssh_keygen: label.to_owned(),
        };
        match git {
            Loadable::Loading => same("Checking…"),
            Loadable::Error(GitError::GitNotFound(_)) => Self {
                git: "Not found".to_owned(),
                ..same("Unknown")
            },
            Loadable::Error(error) => Self {
                // The full error, which can span lines, is on the notice.
                git: format!(
                    "Could not run git: {}",
                    error.to_string().lines().next().unwrap_or_default()
                ),
                ..same("Unknown")
            },
            Loadable::Ready(git) => {
                let path = |path: &Option<PathBuf>| {
                    path.as_ref()
                        .map_or_else(|| "Not found".to_owned(), |path| path.display().to_string())
                };
                let version = if git.version.is_supported() {
                    git.version.text.clone()
                } else {
                    format!(
                        "{} (older than {})",
                        git.version.text,
                        GitVersion::minimum_text()
                    )
                };
                Self {
                    git: git.git_path.display().to_string(),
                    version,
                    gpg: path(&git.gpg_path),
                    ssh_keygen: path(&git.ssh_keygen_path),
                }
            }
        }
    }
}

/// Non-blocking: an old git keeps working, but the user should update it.
pub(crate) fn outdated_git(s: &Ui, k: &Kit) -> Option<El> {
    let Loadable::Ready(git) = &s.state.git else {
        return None;
    };
    if git.version.is_supported() || s.git_notice_closed {
        return None;
    }
    let message = format!(
        "Git {} is older than {}. Tafit still works, but some features may not.",
        git.version.text,
        GitVersion::minimum_text()
    );
    let toast = Toast::new("git-outdated-notice", "Please update Git")
        .description(message)
        .lead(rune::icon(&crate::icons::WARNING, 16.0).color(k.c.warning.fg))
        .closable()
        .on_close(|s: &mut Ui| s.git_notice_closed = true)
        .action(k, "Download Git", true, |_: &mut Ui, tx: &mut Tx| {
            open_git_download(tx);
        })
        .build(k);
    Some(ToastRegion::new().toast(toast).build(k))
}

/// Blocking: shown in place of the Repository Tab when git can't be run.
/// A missing git gets install instructions; a broken one gets its error.
pub(crate) fn git_unavailable(error: &GitError, k: &Kit) -> El {
    let minimum = GitVersion::minimum_text();
    let missing = matches!(error, GitError::GitNotFound(_));
    let (key, title, advice) = if missing {
        (
            "git-missing-notice",
            "Git is required",
            format!(
                "Tafit runs your system's git for commits, pushes and other changes. Install Git {minimum} or newer and make sure `git` is on your PATH, then restart Tafit."
            ),
        )
    } else {
        (
            "git-error-notice",
            "Git could not be run",
            format!(
                "Check that `git --version` works in a terminal; if it doesn't, reinstall Git {minimum} or newer. Then restart Tafit."
            ),
        )
    };
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .center()
        .gap(10.0)
        .padding(24.0)
        .child(text(title).font(k.ty.body_medium).color(k.c.text_strong))
        .child(
            text(error.to_string())
                .font(k.ty.ui)
                .color(k.c.danger.fg)
                .wrap(),
        )
        .child(text(advice).font(k.ty.ui).color(k.c.text_muted).wrap())
        .children(missing.then(|| text(install_hint()).font(k.ty.ui).color(k.c.text_muted)))
        .child(download_git_button(k))
}

fn download_git_button(k: &Kit) -> El {
    Button::new("Download Git")
        .on_press_tx(|_: &mut Ui, tx: &mut Tx| open_git_download(tx))
        .build(k)
        .key("download-git")
}

fn open_git_download(tx: &mut Tx) {
    if let Err(err) = tx.open_url(GIT_DOWNLOAD_URL) {
        eprintln!(
            "{}: could not open {GIT_DOWNLOAD_URL}: {err}",
            tafit_base::APP_NAME
        );
    }
}

fn install_hint() -> &'static str {
    if cfg!(target_os = "windows") {
        "On Windows: install Git for Windows, or run `winget install --id Git.Git -e`."
    } else if cfg!(target_os = "macos") {
        "On macOS: run `xcode-select --install`, or `brew install git`."
    } else {
        "On Linux: install git with your package manager, e.g. `sudo apt install git`."
    }
}
