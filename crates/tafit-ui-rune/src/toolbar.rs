//! The toolbar under the tab bar: the Active Repository, where its HEAD is,
//! and how many files changed in each group.

use rune::{El, icon, row, text};
use rune_kit::Kit;
use tafit_core::Head;
use tafit_state::{Loadable, Page};

use crate::Ui;

pub(crate) fn toolbar(s: &Ui, k: &Kit) -> Option<El> {
    let on_tab = s
        .state
        .window(s.window)
        .is_some_and(|window| window.page == Page::RepositoryTab);
    let repo = s.state.active_in(s.window).filter(|_| on_tab)?;
    let Loadable::Ready(info) = &repo.open else {
        return None;
    };
    let label = |value: String, key: &str| text(value).key(key).font(k.ty.ui).color(k.c.text_muted);
    let name = row()
        .gap(6.0)
        .items_center()
        .child(icon(&rune_kit::glyphs::FOLDER, 16.0).color(k.c.icon_base))
        .child(
            text(&info.name)
                .key("toolbar-repository")
                .font(k.ty.ui)
                .color(k.c.text_strong),
        );
    let head = match &info.head {
        Head::Branch(branch) => branch.clone(),
        Head::Unborn(branch) => format!("{branch} (no commits yet)"),
        Head::Detached(commit) => format!("HEAD detached at {commit}"),
    };
    let changes = match &repo.status {
        Loadable::Ready(status) if status.is_clean() => "No changes".to_owned(),
        Loadable::Ready(status) => format!(
            "{} staged · {} unstaged · {} untracked",
            status.staged.len(),
            status.unstaged.len(),
            status.untracked.len()
        ),
        Loadable::Loading => "Reading changes…".to_owned(),
        Loadable::Error(_) => "Could not read the changes".to_owned(),
    };
    Some(
        row()
            .key("toolbar")
            .w_full()
            .h(40.0)
            .px(12.0)
            .gap(20.0)
            .items_center()
            .fill(k.c.bg_base)
            .child(name)
            .child(label(head, "toolbar-head"))
            .child(label(changes, "toolbar-changes")),
    )
}
