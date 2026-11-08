//! Icons tafit needs that `rune-kit`'s glyphs don't have, drawn on the same
//! 16-point grid with 1-point strokes.

use rune::Icon;

/// A warning: an exclamation mark in a triangle.
pub(crate) static WARNING: Icon = Icon {
    name: "tafit-warning",
    view: 16.0,
    body: r#"<path d="M8 1.75L14.75 13.75H1.25Z" stroke="currentColor" stroke-linejoin="round"/><path d="M8 6V9.5" stroke="currentColor" stroke-linecap="round"/><circle cx="8" cy="11.75" r="0.75" fill="currentColor"/>"#,
};
