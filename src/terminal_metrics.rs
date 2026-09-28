//! Shared terminal cell height for layout, painting and pointer coordinates.
use gpui::{Window, font, px};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "f32", into = "f32")]
pub(crate) struct LineHeight(f32);

impl Default for LineHeight {
    fn default() -> Self {
        Self(1.2)
    }
}

impl From<f32> for LineHeight {
    fn from(value: f32) -> Self {
        Self(if value.is_finite() {
            value.clamp(1., 2.)
        } else {
            Self::default().factor()
        })
    }
}

impl From<LineHeight> for f32 {
    fn from(value: LineHeight) -> Self {
        value.0
    }
}

impl LineHeight {
    pub fn factor(self) -> f32 {
        self.0
    }

    pub fn measure(self, window: &Window, family: &str, size: f32) -> f32 {
        let text = window.text_system();
        let face = text.resolve_font(&font(family.to_owned()));
        let natural =
            f32::from(text.ascent(face, px(size))) + f32::from(text.descent(face, px(size))).abs();
        self.cell_height(natural, window.scale_factor())
    }

    fn cell_height(self, natural: f32, scale: f32) -> f32 {
        // xterm.js 6 WebGL: ceil font metrics, then floor scaled cell size,
        // both in device pixels. Native font metrics remain platform-specific.
        ((natural * scale).ceil() * self.0).floor().max(1.) / scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_height_rounds_in_device_pixels() {
        for (natural, scale, factor, expected) in [
            (16.4, 1., 1., 17.),
            (16.4, 1., 1.2, 20.),
            (16.4, 2., 1., 16.5),
            (16.4, 2., 1.2, 19.5),
            (21.75, 1., 1.2, 26.),
        ] {
            assert_eq!(
                LineHeight::from(factor).cell_height(natural, scale),
                expected
            );
        }
    }

    #[test]
    fn line_height_preferences_validate_and_round_trip() {
        assert_eq!(LineHeight::default().factor(), 1.2);
        for (value, expected) in [(0., 1.), (5., 2.), (1., 1.), (1.2, 1.2), (f32::NAN, 1.2)] {
            let value = LineHeight::from(value);
            assert_eq!(value.factor(), expected);
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<LineHeight>(&json).unwrap(), value);
        }
    }
}
