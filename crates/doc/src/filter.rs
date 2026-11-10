//! Filters on movie clip and button instances, as Animate's Filters panel: Blur, Drop Shadow,
//! Glow and Adjust Color. Sizes are in stage pixels (they don't scale with the instance, as in
//! Animate); they scale with the view.

use crate::Rgba;
use serde::{Deserialize, Serialize};

/// Blur passes: Low, Medium, High (each pass is a box blur; three approach a Gaussian).
pub const MAX_QUALITY: u8 = 3;
/// Largest blur, in stage pixels (Animate's limit).
pub const MAX_BLUR: f32 = 255.0;

fn one() -> u8 {
    1
}
fn full() -> f32 {
    1.0
}
fn black() -> Rgba {
    Rgba::BLACK
}
fn angle() -> f32 {
    45.0
}
fn four() -> f32 {
    4.0
}
fn five() -> f32 {
    5.0
}
fn zero() -> f32 {
    0.0
}
fn half() -> f32 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    Blur {
        #[serde(default = "five")]
        blur_x: f32,
        #[serde(default = "five")]
        blur_y: f32,
        #[serde(default = "one")]
        quality: u8,
    },
    DropShadow {
        #[serde(default = "five")]
        blur_x: f32,
        #[serde(default = "five")]
        blur_y: f32,
        /// Opacity multiplier of the shadow (1 = 100%).
        #[serde(default = "full")]
        strength: f32,
        #[serde(default = "one")]
        quality: u8,
        /// Degrees, clockwise from the right (45 = down and right).
        #[serde(default = "angle")]
        angle: f32,
        #[serde(default = "four")]
        distance: f32,
        #[serde(default = "black")]
        color: Rgba,
        #[serde(default)]
        inner: bool,
        #[serde(default)]
        knockout: bool,
        #[serde(default)]
        hide_object: bool,
        /// Grows (positive) or shrinks (negative) the shadow's silhouette before the blur,
        /// in stage pixels — Moho's shadow `expansion`.
        #[serde(default, skip_serializing_if = "is_zero_f")]
        expansion: f32,
        /// Grain mixed into the shadow, `0..=1` (Moho's shadow `noise_amp`).
        #[serde(default, skip_serializing_if = "is_zero_f")]
        noise: f32,
        /// Hard-edged shadow: the blurred coverage quantises to on/off (Moho's shadow
        /// `threshold`).
        #[serde(default)]
        threshold: bool,
    },
    Glow {
        #[serde(default = "five")]
        blur_x: f32,
        #[serde(default = "five")]
        blur_y: f32,
        #[serde(default = "full")]
        strength: f32,
        #[serde(default = "one")]
        quality: u8,
        #[serde(default = "black")]
        color: Rgba,
        #[serde(default)]
        inner: bool,
        #[serde(default)]
        knockout: bool,
    },
    /// Each in −100..=100 (hue in −180..=180 degrees), 0 = unchanged.
    AdjustColor {
        #[serde(default)]
        brightness: f32,
        #[serde(default)]
        contrast: f32,
        #[serde(default)]
        saturation: f32,
        #[serde(default)]
        hue: f32,
    },
    /// Film grain mixed over the artwork's colour, `0..=1` (Moho's layer `noise`). The grain
    /// is deterministic: the same pixel always gets the same speckle, so renders are stable.
    Noise {
        #[serde(default = "half")]
        amount: f32,
    },
    /// Mosaic: the artwork averages into blocks of `size` stage pixels (Moho's layer
    /// `pixelation`).
    Pixelate {
        #[serde(default = "four")]
        size: f32,
    },
    /// The artwork's colours snap to black or white by their luminance against `level`
    /// (`0..=1`; Moho's layer `threshold`); alpha is kept.
    Threshold {
        #[serde(default = "half")]
        level: f32,
    },
}

fn is_zero_f(v: &f32) -> bool {
    *v == 0.0
}

impl Filter {
    /// The filter's name in Animate's Filters menu.
    pub fn label(&self) -> &'static str {
        match self {
            Filter::Blur { .. } => "Blur",
            Filter::DropShadow { .. } => "Drop Shadow",
            Filter::Glow { .. } => "Glow",
            Filter::AdjustColor { .. } => "Adjust Color",
            Filter::Noise { .. } => "Noise",
            Filter::Pixelate { .. } => "Pixelate",
            Filter::Threshold { .. } => "Threshold",
        }
    }

    /// A new filter of `kind` (`blur`, `drop_shadow`, `glow`, `adjust_color`, `noise`,
    /// `pixelate`, `threshold`) with Animate's defaults.
    pub fn new(kind: &str) -> Option<Filter> {
        Some(match kind {
            "blur" => Filter::Blur { blur_x: five(), blur_y: five(), quality: one() },
            "drop_shadow" => Filter::DropShadow { blur_x: five(), blur_y: five(), strength: full(), quality: one(), angle: angle(), distance: four(), color: black(), inner: false, knockout: false, hide_object: false, expansion: zero(), noise: zero(), threshold: false },
            "glow" => Filter::Glow { blur_x: five(), blur_y: five(), strength: full(), quality: one(), color: Rgba::hex(0xff0000), inner: false, knockout: false },
            "adjust_color" => Filter::AdjustColor { brightness: 0.0, contrast: 0.0, saturation: 0.0, hue: 0.0 },
            "noise" => Filter::Noise { amount: half() },
            "pixelate" => Filter::Pixelate { size: four() },
            "threshold" => Filter::Threshold { level: half() },
            _ => return None,
        })
    }

    /// The kinds [`Filter::new`] makes, in Animate's menu order.
    pub const KINDS: [(&'static str, &'static str); 7] =
        [("drop_shadow", "Drop Shadow"), ("blur", "Blur"), ("glow", "Glow"), ("adjust_color", "Adjust Color"), ("noise", "Noise"), ("pixelate", "Pixelate"), ("threshold", "Threshold")];

    /// The same filter with every value finite and in range.
    pub fn sanitized(&self) -> Filter {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let blur = |v: f32| fin(v, 0.0).clamp(0.0, MAX_BLUR);
        let q = |v: u8| v.clamp(1, MAX_QUALITY);
        let strength = |v: f32| fin(v, 1.0).clamp(0.0, 10.0);
        let unit = |v: f32, d: f32| fin(v, d).clamp(0.0, 1.0);
        match self {
            Filter::Blur { blur_x, blur_y, quality } => Filter::Blur { blur_x: blur(*blur_x), blur_y: blur(*blur_y), quality: q(*quality) },
            Filter::DropShadow { blur_x, blur_y, strength: s, quality, angle, distance, color, inner, knockout, hide_object, expansion, noise, threshold } => Filter::DropShadow {
                blur_x: blur(*blur_x),
                blur_y: blur(*blur_y),
                strength: strength(*s),
                quality: q(*quality),
                angle: fin(*angle, 45.0).rem_euclid(360.0),
                distance: fin(*distance, 0.0).clamp(-1000.0, 1000.0),
                color: color.sanitized(),
                inner: *inner,
                knockout: *knockout,
                hide_object: *hide_object,
                expansion: fin(*expansion, 0.0).clamp(-100.0, 100.0),
                noise: unit(*noise, 0.0),
                threshold: *threshold,
            },
            Filter::Glow { blur_x, blur_y, strength: s, quality, color, inner, knockout } => {
                Filter::Glow { blur_x: blur(*blur_x), blur_y: blur(*blur_y), strength: strength(*s), quality: q(*quality), color: color.sanitized(), inner: *inner, knockout: *knockout }
            }
            Filter::AdjustColor { brightness, contrast, saturation, hue } => Filter::AdjustColor {
                brightness: fin(*brightness, 0.0).clamp(-100.0, 100.0),
                contrast: fin(*contrast, 0.0).clamp(-100.0, 100.0),
                saturation: fin(*saturation, 0.0).clamp(-100.0, 100.0),
                hue: fin(*hue, 0.0).clamp(-180.0, 180.0),
            },
            Filter::Noise { amount } => Filter::Noise { amount: unit(*amount, 0.0) },
            Filter::Pixelate { size } => Filter::Pixelate { size: fin(*size, four()).clamp(1.0, 256.0) },
            Filter::Threshold { level } => Filter::Threshold { level: unit(*level, 0.5) },
        }
    }

    /// How far the filter can spread pixels beyond the artwork, in stage pixels.
    pub fn margin(&self) -> f32 {
        match self.sanitized() {
            Filter::Blur { blur_x, blur_y, quality } => blur_x.max(blur_y) * 0.5 * f32::from(quality) + 1.0,
            Filter::DropShadow { blur_x, blur_y, quality, distance, expansion, inner, .. } => {
                if inner {
                    0.0
                } else {
                    blur_x.max(blur_y) * 0.5 * f32::from(quality) + distance.abs() + expansion.abs() + 1.0
                }
            }
            Filter::Glow { blur_x, blur_y, quality, inner, .. } => if inner { 0.0 } else { blur_x.max(blur_y) * 0.5 * f32::from(quality) + 1.0 },
            Filter::AdjustColor { .. } => 0.0,
            Filter::Noise { .. } | Filter::Pixelate { .. } | Filter::Threshold { .. } => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_default_like_animate_and_stay_in_range() {
        assert_eq!(Filter::new("blur"), Some(Filter::Blur { blur_x: 5.0, blur_y: 5.0, quality: 1 }));
        let Some(Filter::DropShadow { angle, distance, color, strength, expansion, noise, threshold, .. }) = Filter::new("drop_shadow") else { panic!() };
        assert_eq!((angle, distance, color, strength, expansion, noise, threshold), (45.0, 4.0, Rgba::BLACK, 1.0, 0.0, 0.0, false));
        assert!(Filter::new("sparkle").is_none());
        let wild = Filter::Blur { blur_x: f32::NAN, blur_y: 1e9, quality: 9 }.sanitized();
        assert_eq!(wild, Filter::Blur { blur_x: 0.0, blur_y: MAX_BLUR, quality: 3 });
        assert_eq!(Filter::Glow { blur_x: 10.0, blur_y: 4.0, strength: 1.0, quality: 2, color: Rgba::WHITE, inner: false, knockout: false }.margin(), 11.0);
        let json = serde_json::to_string(&Filter::new("adjust_color").unwrap()).unwrap();
        assert_eq!(json, r#"{"type":"adjust_color","brightness":0.0,"contrast":0.0,"saturation":0.0,"hue":0.0}"#);
    }

    #[test]
    fn moho_layer_filters_sanitized() {
        // New variants default and clamp; a wild DropShadow's expansion/noise/threshold stay
        // in range.
        assert_eq!(Filter::new("noise"), Some(Filter::Noise { amount: 0.5 }));
        assert_eq!(Filter::new("pixelate"), Some(Filter::Pixelate { size: 4.0 }));
        assert_eq!(Filter::new("threshold"), Some(Filter::Threshold { level: 0.5 }));
        let wild = Filter::Noise { amount: f32::NAN }.sanitized();
        assert_eq!(wild, Filter::Noise { amount: 0.0 });
        let wild = Filter::Pixelate { size: 1e6 }.sanitized();
        assert_eq!(wild, Filter::Pixelate { size: 256.0 });
        let ds = Filter::DropShadow { blur_x: 5.0, blur_y: 5.0, strength: 1.0, quality: 1, angle: 45.0, distance: 4.0, color: Rgba::BLACK, inner: false, knockout: false, hide_object: false, expansion: 6.0, noise: 0.3, threshold: true };
        assert_eq!(ds.sanitized().margin(), 5.0 * 0.5 + 4.0 + 6.0 + 1.0);
        // Old files (no expansion/noise/threshold keys) load with the defaults.
        let old: Filter = serde_json::from_str(r#"{"type":"drop_shadow","blur_x":3.0,"blur_y":3.0,"strength":1.0,"quality":1,"angle":45.0,"distance":4.0,"color":{"r":0.0,"g":0.0,"b":0.0,"a":1.0},"inner":false,"knockout":false,"hide_object":false}"#).unwrap();
        let Filter::DropShadow { expansion, noise, threshold, .. } = old.sanitized() else { panic!() };
        assert_eq!((expansion, noise, threshold), (0.0, 0.0, false));
    }
}
