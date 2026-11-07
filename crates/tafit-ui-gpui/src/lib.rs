//! The GPUI host: opens a GPUI window per store Window and draws `AppState`
//! snapshots. Holds no app state of its own (only which GPUI window shows
//! which store Window, and where lists are scrolled); every change goes
//! through the store.

mod diff_view;
mod host;
mod open_repository;
mod repository_tabs;
mod status_panel;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, IntoElement, KeyBinding, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use tafit_core::{GitEnvironment, GitError, GitVersion, Head};
use tafit_state::{
    AppState, AppStore, DiffOptions, Loadable, Msg, Page, RepoId, RepoState, StoreEvents, WindowId,
};
use tafit_ui_kit::text::{TextChanged, TextState, bind_text_keys, new_text_state};
use tafit_ui_kit::{Theme, button, fonts, tab_bar};

use crate::repository_tabs::tab_bar_button;

gpui::actions!(
    tafit,
    [
        /// Commit what is staged with the typed message.
        CommitStaged,
    ]
);

/// Binds tafit's keys: text editing, and Ctrl/Cmd-Enter to commit.
pub fn bind_keys(cx: &mut App) {
    bind_text_keys(cx);
    cx.bind_keys([KeyBinding::new(
        "secondary-enter",
        CommitStaged,
        Some(status_panel::COMMIT_BOX_CONTEXT),
    )]);
}

/// Where users get git.
const GIT_DOWNLOAD_URL: &str = "https://git-scm.com/downloads";

/// Runs the windowed app until its last Window is closed.
pub fn run(store: Arc<AppStore>, events: StoreEvents) {
    application().run(move |cx: &mut App| {
        if let Err(err) = fonts::register(cx) {
            // Still usable: text falls back to the system's fonts.
            eprintln!(
                "{}: could not load bundled fonts: {err:#}",
                tafit_base::APP_NAME
            );
        }
        bind_keys(cx);
        host::Host::start(store, events, cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// The window icon. Only Linux reads it from here; Windows ignores it and
/// takes the exe's icon resource, macOS the app bundle's icon.
fn window_icon() -> Option<Arc<image::RgbaImage>> {
    const PNG: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/tafit-window-icon.png"
    ));
    // A missing icon is cosmetic; never fail startup over it.
    let icon = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?;
    Some(Arc::new(icon.into_rgba8()))
}

#[cfg(target_os = "windows")]
fn application() -> gpui::Application {
    // wgpu (DX12) where available, falling back to DX11.
    gpui_platform::application_with_windows_renderer(gpui_platform::WindowsRendererOptions {
        preference: gpui_platform::WindowsRendererPreference::Auto,
        on_event: None,
    })
}

#[cfg(not(target_os = "windows"))]
fn application() -> gpui::Application {
    gpui_platform::application()
}

/// The Window's root view: a tab bar of Repository Tabs over the active one.
pub struct TafitView {
    /// The store Window this view shows.
    window: WindowId,
    store: Arc<AppStore>,
    state: Arc<AppState>,
    theme: Theme,
    /// The commit message box. Every edit goes to the store; the box takes
    /// the store's message only when the Active Repository changes or a
    /// commit is made, so a snapshot lagging behind typing can't undo keys.
    commit_message: Entity<TextState>,
    /// The Repository Tab and commit count the box last took its text from.
    commit_message_source: Option<(RepoId, u64)>,
    _commit_message_edits: Subscription,
    /// The diff's scroll position; the only state the view keeps itself.
    diff_lists: diff_view::DiffLists,
}

impl TafitView {
    /// Shows the store's `window`, starting from the latest snapshot. The
    /// app's host hands it each newer one (`set_state`); tests, under GPUI's
    /// scheduler, which another thread can't wake, keep the first.
    pub fn new(store: Arc<AppStore>, window: WindowId, cx: &mut Context<Self>) -> Self {
        let commit_message = new_text_state(cx);
        let edits = cx.subscribe(&commit_message, |view, message, _: &TextChanged, cx| {
            if let Some(repo) = view.state.active_in(view.window) {
                let repo_id = repo.id;
                view.store.dispatch(Msg::EditCommitMessage {
                    repo_id,
                    message: message.read(cx).as_str().to_owned(),
                });
            }
        });
        let mut view = Self {
            window,
            state: store.snapshot(),
            store,
            theme: Theme::dark(),
            commit_message,
            commit_message_source: None,
            _commit_message_edits: edits,
            diff_lists: diff_view::DiffLists::default(),
        };
        view.sync_commit_message(cx);
        view
    }

    /// Shows a newer snapshot.
    pub(crate) fn set_state(&mut self, state: Arc<AppState>, cx: &mut Context<Self>) {
        self.state = state;
        self.sync_commit_message(cx);
        cx.notify();
    }

    fn show_page(&self, page: Page) {
        self.store.dispatch(Msg::ShowPage {
            window: self.window,
            page,
        });
    }

    /// Shows the Active Repository's message after a tab switch or a commit.
    fn sync_commit_message(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.state.active_in(self.window) else {
            return;
        };
        let source = Some((repo.id, repo.commits_made));
        if self.commit_message_source == source {
            return;
        }
        self.commit_message_source = source;
        let message = repo.commit_message.clone();
        self.commit_message.update(cx, |text, cx| {
            if text.as_str() != message {
                text.emplace(&message, cx);
            }
        });
    }
}

impl Render for TafitView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let state = &self.state;
        let page = state
            .window(self.window)
            .map_or(Page::RepositoryTab, |window| window.page);
        let tabs = repository_tabs::repository_tabs(state, self.window, theme, cx);
        let buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .child(
                tab_bar_button("+", "open-repository-button", theme)
                    .on_click(cx.listener(|view, _, _, _| view.show_page(Page::OpenRepository))),
            )
            .child(
                tab_bar_button("New Window", "new-window-button", theme)
                    .on_click(cx.listener(|view, _, _, _| view.store.dispatch(Msg::OpenWindow))),
            )
            .child(
                tab_bar_button("Settings", "settings-button", theme)
                    .on_click(cx.listener(|view, _, _, _| view.show_page(Page::Settings))),
            );
        let outdated_git = match &state.git {
            Loadable::Ready(git) if !git.version.is_supported() => {
                Some(outdated_git_banner(&git.version, theme))
            }
            _ => None,
        };
        // With no Repository Tab, the Window offers to open one.
        let active = state
            .active_in(self.window)
            .filter(|_| page == Page::RepositoryTab);
        let body = match (page, active) {
            (Page::Settings, _) => settings_page(state, theme, cx).into_any_element(),
            // Without git nothing can be written, so this blocks the Window.
            _ if let Loadable::Error(error) = &state.git => {
                git_unavailable_notice(error, theme).into_any_element()
            }
            (_, None) => open_repository::open_repository_page(state, self.window, theme, cx)
                .into_any_element(),
            (_, Some(repo)) => repository_tab_body(
                repo,
                state.diff_options,
                &mut self.diff_lists,
                &self.store,
                &self.commit_message,
                theme,
                cx,
            )
            .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .font_family(fonts::UI_FONT_FAMILY)
            .text_color(theme.text)
            .text_sm()
            .child(tab_bar(theme).child(tabs).child(buttons))
            .children(outdated_git)
            .child(body)
    }
}

/// Non-blocking: an old git keeps working, but the user should update it.
fn outdated_git_banner(version: &GitVersion, theme: &Theme) -> impl IntoElement {
    div()
        .debug_selector(|| "git-outdated-notice".to_owned())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(8.))
        .border_b_1()
        .border_color(theme.border)
        .text_color(theme.warning)
        .child(div().flex_1().child(format!(
            "Git {} is older than {}. tafit still works, but some features may not. Please update Git.",
            version.text,
            GitVersion::minimum_text()
        )))
        .child(download_git_button(theme))
}

/// Blocking: shown in place of the Repository Tab when git can't be run.
/// A missing git gets install instructions; a broken one gets its error.
fn git_unavailable_notice(error: &GitError, theme: &Theme) -> impl IntoElement {
    let minimum = GitVersion::minimum_text();
    let (selector, title, advice) = match error {
        GitError::GitNotFound(_) => (
            "git-missing-notice",
            "Git is required",
            format!(
                "tafit runs your system's git for commits, pushes and other changes. Install Git {minimum} or newer and make sure `git` is on your PATH, then restart tafit."
            ),
        ),
        _ => (
            "git-error-notice",
            "Git could not be run",
            format!(
                "Check that `git --version` works in a terminal; if it doesn't, reinstall Git {minimum} or newer. Then restart tafit."
            ),
        ),
    };
    let install_hint = matches!(error, GitError::GitNotFound(_))
        .then(|| div().text_color(theme.text_muted).child(install_hint()));
    div()
        .debug_selector(move || selector.to_owned())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .p(px(24.))
        .child(div().text_xl().child(title))
        .child(div().text_color(theme.error).child(error.to_string()))
        .child(div().text_color(theme.text_muted).child(advice))
        .children(install_hint)
        .child(download_git_button(theme))
}

fn download_git_button(theme: &Theme) -> impl IntoElement {
    button("Download Git", theme)
        .id("download-git")
        .on_click(|_, _, cx| cx.open_url(GIT_DOWNLOAD_URL))
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

/// The settings page: for now, the programs tafit runs.
fn settings_page(state: &AppState, theme: &Theme, cx: &mut Context<TafitView>) -> impl IntoElement {
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
    div()
        .debug_selector(|| "settings-page".to_owned())
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(24.))
        .child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().text_xl().child("Settings"))
                .child(
                    button("Close", theme)
                        .id("close-settings")
                        .on_click(cx.listener(|view, _, _, _| view.show_page(Page::RepositoryTab))),
                ),
        )
        .child(
            div()
                .pt(px(8.))
                .text_color(theme.text_muted)
                .child("Programs"),
        )
        .children(
            rows.into_iter()
                .map(|(label, value, selector)| setting_row(label, value, selector, theme)),
        )
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

fn setting_row(
    label: &'static str,
    value: String,
    selector: &'static str,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.))
        .child(
            div()
                .w(px(120.))
                .flex_none()
                .text_color(theme.text_muted)
                .child(label),
        )
        .child(
            div()
                .debug_selector(move || selector.to_owned())
                .font_family(fonts::MONOSPACE_FONT_FAMILY)
                .child(value),
        )
}

#[allow(clippy::too_many_arguments)]
fn repository_tab_body(
    repo: &RepoState,
    diff_options: DiffOptions,
    diff_lists: &mut diff_view::DiffLists,
    store: &Arc<AppStore>,
    commit_message: &Entity<TextState>,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement {
    match &repo.open {
        Loadable::Loading => centered_message("Opening…", theme.text_muted).into_any_element(),
        Loadable::Error(message) => div()
            .debug_selector(|| "repository-error".to_owned())
            .child(centered_message(message.clone(), theme.error))
            .size_full()
            .into_any_element(),
        Loadable::Ready(info) => div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(6.))
                    .p(px(24.))
                    .child(
                        div()
                            .debug_selector(|| "repository-name".to_owned())
                            .text_xl()
                            .child(info.name.clone()),
                    )
                    .child(
                        div()
                            .debug_selector(|| "repository-head".to_owned())
                            .text_color(theme.text_muted)
                            .child(head_label(&info.head)),
                    ),
            )
            .child(
                // The change list on the left, the selected file's diff beside it.
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().w(px(380.)).flex_none().flex().flex_col().child(
                        status_panel::status_panel(repo, store, commit_message, theme, cx),
                    ))
                    .child(diff_view::diff_view(
                        repo,
                        diff_options,
                        diff_lists,
                        theme,
                        cx,
                    )),
            )
            .into_any_element(),
    }
}

fn head_label(head: &Head) -> String {
    match head {
        Head::Branch(branch) => format!("On branch {branch}"),
        Head::Unborn(branch) => format!("On branch {branch} (no commits yet)"),
        Head::Detached(commit) => format!("HEAD detached at {commit}"),
    }
}

fn centered_message(text: impl Into<SharedString>, color: gpui::Hsla) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes() {
        let icon = super::window_icon().expect("the window icon decodes");
        assert_eq!(icon.dimensions(), (32, 32));
    }
}
