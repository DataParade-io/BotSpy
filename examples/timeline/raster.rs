//! Rasterize the figure SVG to a PNG with the pure-Rust `resvg` crate.
//! System fonts are loaded through its font database, with a sans-serif
//! fallback chosen from the families actually installed.

use resvg::{tiny_skia, usvg};

/// Sans-serif families to prefer, in order, for the generic `sans-serif`.
const SANS_CANDIDATES: [&str; 7] = [
    "Helvetica Neue",
    "Helvetica",
    "Arial",
    "Liberation Sans",
    "DejaVu Sans",
    "Noto Sans",
    "Segoe UI",
];

fn font_database() -> usvg::fontdb::Database {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    let installed = |name: &str| {
        fonts
            .faces()
            .any(|face| face.families.iter().any(|(family, _)| family == name))
    };
    if let Some(name) = SANS_CANDIDATES.iter().find(|name| installed(name)) {
        fonts.set_sans_serif_family(*name);
    }
    fonts
}

/// The PNG bytes of `svg` drawn at `scale` times its own size.
pub fn png(svg: &str, scale: f32) -> Result<Vec<u8>, String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err("--scale must be a positive number".to_string());
    }
    let options = usvg::Options {
        fontdb: std::sync::Arc::new(font_database()),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).map_err(|error| error.to_string())?;
    let size = tree.size();
    let (width, height) = (
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    );
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("cannot make a {width}x{height} image"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|error| error.to_string())
}
