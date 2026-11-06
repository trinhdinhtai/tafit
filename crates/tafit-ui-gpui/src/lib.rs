//! The GPUI host: opens the Window and draws `AppState` snapshots. Holds no
//! app state of its own; every change goes through the store.

mod status_panel;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, IntoElement, Render, SharedString, Task, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use tafit_core::{GitEnvironment, GitError, GitVersion, Head};
use tafit_state::{AppState, AppStore, Loadable, Msg, RepoState, StoreEvents};
use tafit_ui_kit::{Theme, button, fonts, tab, tab_bar};

/// Where users get git.
const GIT_DOWNLOAD_URL: &str = "https://git-scm.com/downloads";

/// Runs the windowed app until the Window is closed.
pub fn run(store: Arc<AppStore>, events: StoreEvents) {
    application().run(move |cx: &mut App| {
        if let Err(err) = fonts::register(cx) {
            // Still usable: text falls back to the system's fonts.
            eprintln!(
                "{}: could not load bundled fonts: {err:#}",
                tafit_base::APP_NAME
            );
        }
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(tafit_base::APP_NAME.into()),
                    ..Default::default()
                }),
                icon: window_icon(),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| TafitView::new(store, Some(events), cx)),
        )
        .expect("open the main window");
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
    store: Arc<AppStore>,
    state: Arc<AppState>,
    theme: Theme,
    _poller: Option<Task<()>>,
}

impl TafitView {
    /// With `events`, a poller re-reads the snapshot after every store change.
    /// Pass `None` under GPUI's test scheduler, which can't be woken from
    /// another thread.
    pub fn new(store: Arc<AppStore>, events: Option<StoreEvents>, cx: &mut Context<Self>) -> Self {
        let poller = events.map(|events| {
            let store = Arc::clone(&store);
            cx.spawn(async move |view, cx| {
                while events.recv().await.is_ok() {
                    let snapshot = store.snapshot();
                    let updated = view.update(cx, |view, cx| {
                        view.state = snapshot;
                        cx.notify();
                    });
                    if updated.is_err() {
                        break; // The view is gone.
                    }
                }
            })
        });
        Self {
            state: store.snapshot(),
            store,
            theme: Theme::dark(),
            _poller: poller,
        }
    }
}

impl Render for TafitView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let state = &self.state;
        let active = state.active_repo;
        let tabs = state.repos.iter().map(|repo| {
            div()
                .debug_selector(|| "repository-tab".to_owned())
                .h_full()
                .child(tab(tab_label(repo), Some(repo.id) == active, theme))
        });
        let settings_button = div().flex().items_center().px(px(8.)).child(
            button("Settings", theme)
                .id("settings-button")
                .debug_selector(|| "settings-button".to_owned())
                .on_click(cx.listener(|view, _, _, _| view.store.dispatch(Msg::OpenSettings))),
        );
        let outdated_git = match &state.git {
            Loadable::Ready(git) if !git.version.is_supported() => {
                Some(outdated_git_banner(&git.version, theme))
            }
            _ => None,
        };
        let body = if state.settings_open {
            settings_page(state, theme, cx).into_any_element()
        } else if let Loadable::Error(error) = &state.git {
            // Without git nothing can be written, so this blocks the Repository Tab.
            git_unavailable_notice(error, theme).into_any_element()
        } else {
            match state.active() {
                Some(repo) => repository_tab_body(repo, theme, cx).into_any_element(),
                None => centered_message("No Repository open", theme.text_muted).into_any_element(),
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .font_family(fonts::UI_FONT_FAMILY)
            .text_color(theme.text)
            .text_sm()
            .child(
                tab_bar(theme)
                    .children(tabs)
                    .child(div().flex_1())
                    .child(settings_button),
            )
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
                .child(button("Close", theme).id("close-settings").on_click(
                    cx.listener(|view, _, _, _| view.store.dispatch(Msg::CloseSettings)),
                )),
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

fn tab_label(repo: &RepoState) -> SharedString {
    match &repo.open {
        Loadable::Ready(info) => info.name.clone().into(),
        // Not opened (yet): fall back to the folder name.
        _ => repo
            .path
            .file_name()
            .unwrap_or(repo.path.as_os_str())
            .to_string_lossy()
            .into_owned()
            .into(),
    }
}

fn repository_tab_body(
    repo: &RepoState,
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
            .child(status_panel::status_panel(repo, theme, cx))
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
