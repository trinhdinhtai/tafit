//! Fonts bundled into the binary so tafit looks the same on every machine and
//! renders Vietnamese. Both families are under the SIL Open Font License; the
//! license texts sit next to the font files.

use std::borrow::Cow;

use gpui::App;

/// Family of the UI font.
pub const UI_FONT_FAMILY: &str = "Google Sans";
/// Family of the monospace font (code, diffs, hashes, paths).
pub const MONOSPACE_FONT_FAMILY: &str = "Google Sans Code";

macro_rules! font {
    ($path:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/", $path)).as_slice()
    };
}

/// Every bundled face. Each is a variable font covering its family's whole
/// weight range, one upright and one italic per family.
pub const BUNDLED_FONTS: &[&[u8]] = &[
    font!("google_sans/GoogleSans[GRAD,opsz,wght].ttf"),
    font!("google_sans/GoogleSans-Italic[GRAD,opsz,wght].ttf"),
    font!("google_sans_code/GoogleSansCode[MONO,wght].ttf"),
    font!("google_sans_code/GoogleSansCode-Italic[MONO,wght].ttf"),
];

/// Makes the bundled fonts available to GPUI. Call once at startup, before
/// the first window draws.
pub fn register(cx: &App) -> gpui::Result<()> {
    cx.text_system().add_fonts(
        BUNDLED_FONTS
            .iter()
            .map(|bytes| Cow::Borrowed(*bytes))
            .collect(),
    )
}
