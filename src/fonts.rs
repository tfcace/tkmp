//! Embedded fonts, text measurement and the font database resvg draws with.
//!
//! Inter and JetBrains Mono are compiled in so the Mac and the NAS render
//! identically. Measurement reads advance widths straight from the font
//! tables, which keeps Pele's layout and our heading wrapping in agreement
//! with what resvg later draws.

use resvg::usvg::fontdb;
use std::sync::{Arc, LazyLock, OnceLock};
use ttf_parser::Face;

static INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
static DISPLAY_BOLD: &[u8] = include_bytes!("../assets/fonts/InterDisplay-Bold.ttf");
static DISPLAY_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/InterDisplay-SemiBold.ttf");
static MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

pub const SANS: &str = "Inter";
pub const DISPLAY: &str = "Inter Display";
pub const MONO_FAMILY: &str = "JetBrains Mono";

struct Faces {
    regular: Face<'static>,
    semibold: Face<'static>,
    display_bold: Face<'static>,
    display_semibold: Face<'static>,
    mono: Face<'static>,
}

static FACES: LazyLock<Faces> = LazyLock::new(|| Faces {
    regular: Face::parse(INTER, 0).expect("Inter"),
    semibold: Face::parse(INTER_SEMIBOLD, 0).expect("Inter SemiBold"),
    display_bold: Face::parse(DISPLAY_BOLD, 0).expect("Inter Display Bold"),
    display_semibold: Face::parse(DISPLAY_SEMIBOLD, 0).expect("Inter Display SemiBold"),
    mono: Face::parse(MONO, 0).expect("JetBrains Mono"),
});

/// Which face to measure with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Weight {
    Regular,
    SemiBold,
    DisplayBold,
    DisplaySemiBold,
    Mono,
}

fn face(w: Weight) -> &'static Face<'static> {
    let f = &*FACES;
    match w {
        Weight::Regular => &f.regular,
        Weight::SemiBold => &f.semibold,
        Weight::DisplayBold => &f.display_bold,
        Weight::DisplaySemiBold => &f.display_semibold,
        Weight::Mono => &f.mono,
    }
}

/// Pele's measurer contract: style flags are 1 bold, 2 italic, 4 monospace.
pub fn measure(text: &str, size: f32, style: i32) -> f32 {
    let w = if style & 4 != 0 {
        Weight::Mono
    } else if style & 1 != 0 {
        Weight::SemiBold
    } else {
        Weight::Regular
    };
    width(text, size, w)
}

pub fn width(text: &str, size: f32, w: Weight) -> f32 {
    let face = face(w);
    let scale = size / face.units_per_em() as f32;
    let mut total = 0.0;
    for ch in text.chars() {
        total += match face.glyph_index(ch).and_then(|g| face.glyph_hor_advance(g)) {
            Some(adv) => adv as f32 * scale,
            // Glyphs the embedded fonts lack come from a system font; this is
            // close enough for wrapping and layout.
            None if ch.is_whitespace() => size * 0.28,
            None => size * 0.62,
        };
    }
    total
}

/// True when every non-space character exists in the embedded sans fonts.
pub fn covers(text: &str) -> bool {
    let face = &FACES.regular;
    text.chars().all(|c| c.is_whitespace() || face.glyph_index(c).is_some())
}

pub struct Metrics {
    pub cap_height: f32,
}

/// Font metrics in em units (multiply by the font size).
pub fn metrics(w: Weight) -> Metrics {
    let face = face(w);
    let upem = face.units_per_em() as f32;
    Metrics {
        cap_height: face.capital_height().unwrap_or(face.ascender() * 7 / 10) as f32 / upem,
    }
}

fn embedded_db() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    for data in [INTER, INTER_SEMIBOLD, DISPLAY_BOLD, DISPLAY_SEMIBOLD, MONO] {
        db.load_font_source(fontdb::Source::Binary(Arc::new(data)));
    }
    db.set_sans_serif_family(SANS);
    db.set_serif_family(SANS);
    db.set_monospace_family(MONO_FAMILY);
    db
}

static EMBEDDED: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| Arc::new(embedded_db()));
static WITH_SYSTEM: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

/// The fast database: only the embedded fonts.
pub fn embedded() -> Arc<fontdb::Database> {
    EMBEDDED.clone()
}

/// Embedded fonts plus the system's, for text the embedded fonts can't draw
/// (Hebrew, CJK, emoji). Loading system fonts takes tens of milliseconds, so
/// `warm_system` starts it in the background at startup.
pub fn with_system() -> Arc<fontdb::Database> {
    WITH_SYSTEM
        .get_or_init(|| {
            let mut db = embedded_db();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

pub fn warm_system() {
    std::thread::Builder::new()
        .name("fonts".into())
        .spawn(|| {
            with_system();
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_wider_for_longer_text() {
        let a = width("Hello", 16.0, Weight::Regular);
        let b = width("Hello world", 16.0, Weight::Regular);
        assert!(a > 20.0 && b > a);
    }

    #[test]
    fn bold_is_wider_than_regular() {
        assert!(measure("Markdown", 16.0, 1) > measure("Markdown", 16.0, 0));
    }

    #[test]
    fn coverage() {
        assert!(covers("Plan: phase 2 (draft)"));
        assert!(!covers("שלום"));
    }
}
