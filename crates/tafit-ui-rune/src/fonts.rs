//! Fonts bundled into the binary so tafit looks the same on every machine
//! and renders Vietnamese. Both families are under the SIL Open Font
//! License; the license texts sit next to the font files.

macro_rules! font {
    ($path:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tafit-ui-kit/assets/fonts/",
            $path
        ))
        .as_slice()
    };
}

/// Every bundled face: variable fonts covering each family's weights, one
/// upright and one italic per family.
pub(crate) const BUNDLED: &[&[u8]] = &[
    font!("google_sans/GoogleSans[GRAD,opsz,wght].ttf"),
    font!("google_sans/GoogleSans-Italic[GRAD,opsz,wght].ttf"),
    font!("google_sans_code/GoogleSansCode[MONO,wght].ttf"),
    font!("google_sans_code/GoogleSansCode-Italic[MONO,wght].ttf"),
];

/// The UI font and the monospace font (code, diffs, hashes, paths).
pub(crate) const FAMILIES: (&str, &str) = ("Google Sans", "Google Sans Code");
