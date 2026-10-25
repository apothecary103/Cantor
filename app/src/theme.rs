//! Visual design tokens (docs/spec/50-ux.md "Visual direction").
//!
//! Dark first, light supported; neutral graphite surfaces; one accent for
//! selection and focus; a small fixed palette for track colors; radii of
//! 4 px or less; no gradients, blur or shadows (the types below cannot
//! express them).

/// An sRGB color, 0xRRGGBB.
pub type Rgb = u32;

/// Colors and metrics for one appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Window background.
    pub background: Rgb,
    /// Panels and toolbars.
    pub surface: Rgb,
    /// Piano-roll rows for in-scale pitches.
    pub lane: Rgb,
    /// Piano-roll rows for out-of-scale pitches.
    pub lane_alt: Rgb,
    /// Beat lines.
    pub grid: Rgb,
    /// Bar lines.
    pub grid_strong: Rgb,
    /// Primary text.
    pub text: Rgb,
    /// Lane labels and secondary text.
    pub text_quiet: Rgb,
    /// Selection, focus and the playhead.
    pub accent: Rgb,
    /// Pitch error heat (hot end).
    pub heat: Rgb,
    /// Panel separators.
    pub border: Rgb,
    /// Track colors, assigned by track index.
    pub tracks: [Rgb; 6],
    /// Corner radius for notes and controls, px.
    pub radius: f32,
    /// Curve stroke width, px.
    pub curve_width: f32,
    /// Opacity of model-prediction curves (user deltas are opaque).
    pub model_curve_alpha: f32,
}

/// The dark theme (default).
pub const DARK: Theme = Theme {
    background: 0x1b1c1f,
    surface: 0x222327,
    lane: 0x26272c,
    lane_alt: 0x202125,
    grid: 0x2c2d33,
    grid_strong: 0x3a3c43,
    text: 0xd6d8de,
    text_quiet: 0x7f838e,
    accent: 0x4c9aff,
    heat: 0xe5484d,
    border: 0x303137,
    tracks: [0x6f8cf2, 0x5cb88a, 0xd9a24a, 0xc56fc9, 0x4fb4c4, 0xd97a5b],
    radius: 3.0,
    curve_width: 1.5,
    model_curve_alpha: 0.35,
};

/// The light theme.
pub const LIGHT: Theme = Theme {
    background: 0xf4f5f7,
    surface: 0xeaebee,
    lane: 0xffffff,
    lane_alt: 0xf0f1f4,
    grid: 0xe0e2e7,
    grid_strong: 0xc9ccd3,
    text: 0x1f2126,
    text_quiet: 0x6b6f7a,
    accent: 0x1f6fe0,
    heat: 0xd1343a,
    border: 0xd7d9de,
    tracks: [0x4867d6, 0x2f9467, 0xb7801f, 0xa4479f, 0x26899a, 0xbd5a3a],
    radius: 3.0,
    curve_width: 1.5,
    model_curve_alpha: 0.35,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Rgb) -> f64 {
        let ch = |s: u32| {
            let v = f64::from((c >> s) & 0xff) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// The mechanical parts of the visual direction.
    #[test]
    fn themes_follow_the_visual_direction() {
        for t in [DARK, LIGHT] {
            assert!(t.radius <= 4.0, "radii stay at 4 px or below");
            assert_eq!(t.curve_width, 1.5, "curves are 1.5 px");
            assert!(t.model_curve_alpha < 0.6, "model prediction is drawn faint");
            assert!(
                contrast(t.text, t.background) >= 7.0,
                "primary text contrast"
            );
            assert!(
                contrast(t.text_quiet, t.background) >= 3.0,
                "quiet text stays legible"
            );
            assert!(
                contrast(t.accent, t.background) >= 3.0,
                "accent is unmistakable"
            );
            let mut tracks = t.tracks.to_vec();
            tracks.dedup();
            assert_eq!(tracks.len(), 6, "a small fixed palette for track colors");
        }
        assert!(luminance(DARK.background) < 0.05, "dark theme first");
    }
}
