//! Text entry. Wraps gpui-ce's editable text, which brings IME (Vietnamese
//! input included), selection, clipboard and undo; tafit adds the look.

use gpui::{App, ColorExt, Entity, SharedString, prelude::*, px};
use gpui_elements::editable_text::actions::{DEFAULT_INPUT_CONTEXT, default_bindings};
use gpui_elements::editable_text::{self, EditableTextElement};

pub use gpui_elements::editable_text::{
    EditableTextState as TextState, StringStorage as TextStorage, TextChanged,
};

use crate::Theme;

/// Binds the editing keys (arrows, Home/End, clipboard, undo, ...) for every
/// text box. Call once at startup.
pub fn bind_text_keys(cx: &mut App) {
    cx.bind_keys(default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT)));
}

/// An empty text box's state; keep it in the view so the text survives
/// re-renders.
pub fn new_text_state(cx: &mut App) -> Entity<TextState> {
    cx.new(|cx| TextState::new(TextStorage::default(), cx))
}

/// A multi-line text box that wraps its text and scrolls when it overflows.
/// The caller gives it a size.
pub fn text_area(
    id: impl Into<gpui::ElementId>,
    state: &Entity<TextState>,
    placeholder: impl Into<SharedString>,
    theme: &Theme,
) -> EditableTextElement {
    editable_text::text_area(id)
        .state(state.downgrade())
        .placeholder(placeholder)
        .placeholder_color(theme.text_muted)
        .selection_color(theme.accent.opacity(0.35))
        .caret_color(theme.text)
        .caret_blink_interval_500ms()
        .px(px(8.))
        .py(px(6.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_color(theme.text)
        .whitespace_normal()
        .overflow_y_scroll()
}
