//! The bundled fonts must render Vietnamese, and the family names the UI asks
//! for must be the ones the font files declare.

use skrifa::{FontRef, MetadataProvider, string::StringId};
use tafit_ui_kit::fonts::{BUNDLED_FONTS, MONOSPACE_FONT_FAMILY, UI_FONT_FAMILY};

/// Every Vietnamese letter with a diacritic, lower and upper case.
const VIETNAMESE: &str = "ăâđêôơư\
    àảãáạằẳẵắặầẩẫấậèẻẽéẹềểễếệìỉĩíịòỏõóọồổỗốộờởỡớợùủũúụừửữứựỳỷỹýỵ\
    ĂÂĐÊÔƠƯ\
    ÀẢÃÁẠẰẲẴẮẶẦẨẪẤẬÈẺẼÉẸỀỂỄẾỆÌỈĨÍỊÒỎÕÓỌỒỔỖỐỘỜỞỠỚỢÙỦŨÚỤỪỬỮỨỰỲỶỸÝỴ";

fn typographic_family(font: &FontRef) -> String {
    // Static fonts name the family without the weight in the typographic
    // family entry; fall back to the legacy entry when it is absent.
    [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]
        .into_iter()
        .find_map(|id| font.localized_strings(id).english_or_first())
        .map(|name| name.to_string())
        .expect("font has a family name")
}

#[test]
fn every_bundled_font_covers_vietnamese() {
    for (ix, bytes) in BUNDLED_FONTS.iter().enumerate() {
        let font = FontRef::new(bytes).expect("bundled font parses");
        let charmap = font.charmap();
        let missing: String = VIETNAMESE
            .chars()
            .filter(|&ch| charmap.map(ch).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "bundled font #{ix} ({}) has no glyph for: {missing}",
            typographic_family(&font)
        );
    }
}

#[test]
fn the_default_families_are_the_bundled_ones() {
    let families: Vec<String> = BUNDLED_FONTS
        .iter()
        .map(|bytes| typographic_family(&FontRef::new(bytes).expect("bundled font parses")))
        .collect();

    for family in [UI_FONT_FAMILY, MONOSPACE_FONT_FAMILY] {
        let faces = families.iter().filter(|name| *name == family).count();
        // One upright and one italic variable font per family.
        assert_eq!(
            faces, 2,
            "{family}: expected 2 bundled faces, found {faces}"
        );
    }
}
