//! tafit's own widgets and theme tokens. Pure GPUI: never depends on
//! `tafit-core` or `tafit-state`, so widgets take plain data.

pub mod fonts;

use gpui::{Div, Hsla, IntoElement, SharedString, div, prelude::*, px, rgb, rgb_to_hsla};

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

/// One tab; the active one is underlined with the accent color.
pub fn tab(label: impl Into<SharedString>, active: bool, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .px(px(14.))
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
