//! tafit's own widgets and theme tokens. Pure GPUI: never depends on
//! `tafit-core` or `tafit-state`, so widgets take plain data.

pub mod fonts;
pub mod text;

use gpui::{Div, Hsla, SharedString, div, prelude::*, px, rgb, rgb_to_hsla};

/// Color tokens. Widgets read colors from here, never from literals.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub accent: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    /// Additions, e.g. an added file in the change list.
    pub success: Hsla,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            background: rgb_to_hsla(rgb(0x1e1f22)),
            surface: rgb_to_hsla(rgb(0x2b2d30)),
            border: rgb_to_hsla(rgb(0x393b40)),
            text: rgb_to_hsla(rgb(0xdfe1e5)),
            text_muted: rgb_to_hsla(rgb(0x9da0a8)),
            accent: rgb_to_hsla(rgb(0x3574f0)),
            error: rgb_to_hsla(rgb(0xf75464)),
            warning: rgb_to_hsla(rgb(0xe5a23b)),
            success: rgb_to_hsla(rgb(0x5fb865)),
        }
    }
}

/// A horizontal strip that holds `tab`s.
pub fn tab_bar(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .h(px(34.))
        .bg(theme.surface)
        .border_b_1()
        .border_color(theme.border)
}

/// A clickable text button. The caller gives it an id and a click handler.
pub fn button(label: impl Into<SharedString>, theme: &Theme) -> Div {
    let hover = theme.border;
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .text_color(theme.text)
        .cursor_pointer()
        .hover_style(move |style| style.bg(hover))
        .child(label.into())
}

/// A `button` that can't be clicked right now: muted, with no hover.
pub fn disabled_button(label: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .text_color(theme.text_muted)
        .child(label.into())
}

/// A small "×" that closes what it sits on, e.g. a `tab`. The caller gives
/// it an id and a click handler.
pub fn close_button(theme: &Theme) -> Div {
    let hover = theme.border;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(3.))
        .text_xs()
        .text_color(theme.text_muted)
        .cursor_pointer()
        .hover_style(move |style| style.bg(hover))
        .child("×")
}

/// One tab; the active one is underlined with the accent color. Children
/// added by the caller, such as a `close_button`, follow the label.
pub fn tab(label: impl Into<SharedString>, active: bool, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .pl(px(14.))
        .pr(px(8.))
        .h_full()
        .border_b_2()
        .border_color(if active {
            theme.accent
        } else {
            gpui::transparent_black()
        })
        .text_color(if active { theme.text } else { theme.text_muted })
        .child(label.into())
}
