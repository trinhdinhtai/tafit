//! The Rune host (ADR-0006): opens a Rune window per store Window and draws
//! `AppState` snapshots with Rune and `rune-kit`. Holds no app state of its
//! own beyond what each window shows (its text edits, tab drag, diff
//! scroll); every change goes through the store.

mod diff;
mod fonts;
mod host;
mod pages;
mod status;
mod tabs;

use std::sync::Arc;

use rune::{Cx, El, col, row, text};
use rune_kit::Kit;
use rune_kit::shell::TabDragState;
use tafit_core::Head;
use tafit_state::{AppState, AppStore, Loadable, Msg, Page, RepoState, WindowId};

pub use host::run_app as run;

/// What one Window shows: the latest snapshot, and the little UI state the
/// store doesn't keep.
pub(crate) struct Ui {
    shared: Arc<host::Shared>,
    /// The store Window this window shows.
    window: WindowId,
    state: Arc<AppState>,
    /// The `raise_requests` already acted on.
    raised: u64,
    /// A Repository Tab being dragged along the strip.
    tab_drag: TabDragState,
    /// The modifier keys held, for Ctrl/Cmd-click.
    mods: rune::Mods,
    /// The commit message box. Every edit goes to the store.
    commit: rune::TextEdit,
    /// The Repository and commit count the box last took its text from.
    commit_source: Option<(tafit_state::RepoId, u64)>,
    /// The diff view of the selected file.
    diff: diff::ShownDiff,
}

impl Ui {
    fn new(shared: Arc<host::Shared>, window: WindowId) -> Self {
        let state = shared.store.snapshot();
        let raised = state.window(window).map_or(0, |w| w.raise_requests);
        let mut ui = Self {
            shared,
            window,
            state,
            raised,
            tab_drag: TabDragState::default(),
            mods: rune::Mods::default(),
            commit: rune::TextEdit::new(""),
            commit_source: None,
            diff: diff::ShownDiff::default(),
        };
        ui.sync_commit_message();
        ui.sync_diff();
        ui
    }

    fn store(&self) -> &AppStore {
        &self.shared.store
    }

    fn dispatch(&self, msg: Msg) {
        self.store().dispatch(msg);
    }

    fn show_page(&self, page: Page) {
        self.dispatch(Msg::ShowPage {
            window: self.window,
            page,
        });
    }

    /// Takes a newer snapshot, if there is one; true when there was.
    fn refresh(&mut self) -> bool {
        let latest = self.store().snapshot();
        if Arc::ptr_eq(&latest, &self.state) {
            return false;
        }
        self.state = latest;
        self.sync_commit_message();
        self.sync_diff();
        true
    }
}

/// The Window's root: the tab bar over the page it shows.
fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let k = kit(cx.system_dark());
    let mut layer = Vec::new();
    let state = &s.state;
    let page = state
        .window(s.window)
        .map_or(Page::RepositoryTab, |window| window.page);
    // With no Repository Tab, the Window offers to open one.
    let active = state
        .active_in(s.window)
        .filter(|_| page == Page::RepositoryTab);
    let body = match (page, active) {
        (Page::Settings, _) => pages::settings(state, &k),
        // Without git nothing can be written, so this blocks the Window.
        _ if let Loadable::Error(error) = &state.git => pages::git_unavailable(error, &k),
        (_, None) => pages::open_repository(state, s.window, &k),
        (_, Some(repo)) => repository_tab_body(s, repo, &k, &mut layer),
    };
    let root = col()
        .key("window")
        .w_full()
        .h_full()
        .fill(k.c.bg_base)
        .child(tabs::tab_bar(s, cx, &k, &mut layer))
        .children(pages::outdated_git(state, &k))
        .child(body);
    rune_kit::root(root, layer)
}

fn repository_tab_body(s: &Ui, repo: &RepoState, k: &Kit, layer: &mut Vec<El>) -> El {
    match &repo.open {
        Loadable::Loading => centered("Opening…", k.c.text_muted, k),
        Loadable::Error(message) => centered(message, k.c.danger.fg, k).key("repository-error"),
        Loadable::Ready(info) => {
            let header = col()
                .padding(24.0)
                .gap(6.0)
                .child(
                    text(&info.name)
                        .key("repository-name")
                        .font(k.ty.body_medium)
                        .color(k.c.text_strong),
                )
                .child(
                    text(head_label(&info.head))
                        .key("repository-head")
                        .font(k.ty.ui)
                        .color(k.c.text_muted),
                );
            // The change list on the left, the selected file's diff beside it.
            let panes = row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(status::status_panel(s, repo, k))
                .child(diff::diff_pane(s, repo, k, layer));
            col()
                .key("repository")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(header)
                .child(panes)
        }
    }
}

fn head_label(head: &Head) -> String {
    match head {
        Head::Branch(branch) => format!("On branch {branch}"),
        Head::Unborn(branch) => format!("On branch {branch} (no commits yet)"),
        Head::Detached(commit) => format!("HEAD detached at {commit}"),
    }
}

/// `message` in the middle of the space left.
fn centered(message: impl Into<String>, color: rune::Color, k: &Kit) -> El {
    col()
        .w_full()
        .grow(1.0)
        .center()
        .padding(24.0)
        .child(text(message.into()).font(k.ty.ui).color(color).wrap())
}

/// `rune-kit`'s neutral theme with Google Sans at one weight. The kit sets
/// buttons, tabs and titles at 500, which Google Sans draws so much wider
/// and heavier than its 400 at UI sizes that it reads as another font.
fn kit(dark: bool) -> Kit {
    let mut k = Kit::neutral(dark);
    let ty = &mut k.ty;
    ty.ui_medium = ty.ui;
    ty.small_medium = ty.small;
    ty.body_medium = ty.body;
    ty.text_small_medium = ty.text_small;
    ty.text_xsmall_medium = ty.text_xsmall;
    k
}

/// A row of controls with room between the leading and trailing ones.
fn spacer() -> El {
    row().grow(1.0)
}
