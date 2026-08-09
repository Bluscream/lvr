//! Procedurally drawn app/tray icon.
//!
//! Drawing it in code (rather than shipping a PNG) keeps the icon available
//! even when `lvr` runs uninstalled, and lets the tray icon change colour with
//! the VR status.

/// What the icon should communicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Nothing is running.
    Idle,
    /// WiVRn is up, no headset connected.
    Ready,
    /// A headset is connected.
    Connected,
    /// Something needs attention.
    Problem,
}

impl IconState {
    pub fn color(self) -> [u8; 3] {
        match self {
            IconState::Idle => [0x8a, 0x90, 0x99],
            IconState::Ready => [0x4c, 0x8d, 0xff],
            IconState::Connected => [0x3e, 0xcf, 0x6d],
            IconState::Problem => [0xf0, 0x7d, 0x3c],
        }
    }
}

/// Rounded-rectangle coverage test in unit space (0..1 on both axes).
fn in_rounded_rect(x: f32, y: f32, rect: [f32; 4], radius: f32) -> bool {
    let [x0, y0, x1, y1] = rect;
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + radius, x1 - radius);
    let cy = y.clamp(y0 + radius, y1 - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

fn in_ellipse(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> bool {
    let dx = (x - cx) / rx;
    let dy = (y - cy) / ry;
    dx * dx + dy * dy <= 1.0
}

/// Which part of the headset covers this point, if any.
fn sample(x: f32, y: f32) -> Option<Layer> {
    // Head strap: a thin bar behind the visor.
    let strap = in_rounded_rect(x, y, [0.02, 0.40, 0.98, 0.52], 0.05);
    // Visor body.
    let body = in_rounded_rect(x, y, [0.08, 0.28, 0.92, 0.72], 0.16);
    // Nose cut-out at the bottom centre.
    let nose = in_ellipse(x, y, 0.5, 0.78, 0.14, 0.14);
    // Lenses.
    let lens =
        in_ellipse(x, y, 0.31, 0.48, 0.115, 0.095) || in_ellipse(x, y, 0.69, 0.48, 0.115, 0.095);

    if body && !nose {
        if lens {
            return Some(Layer::Lens);
        }
        return Some(Layer::Body);
    }
    if strap && !body {
        return Some(Layer::Strap);
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Body,
    Lens,
    Strap,
}

/// Render an RGBA8 icon of `size` x `size` pixels, 3x3 supersampled.
pub fn render_rgba(size: u32, color: [u8; 3]) -> Vec<u8> {
    const SS: u32 = 3;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let dark = [
        (color[0] as f32 * 0.28) as u8,
        (color[1] as f32 * 0.28) as u8,
        (color[2] as f32 * 0.30) as u8,
    ];
    let strap = [
        (color[0] as f32 * 0.62) as u8,
        (color[1] as f32 * 0.62) as u8,
        (color[2] as f32 * 0.62) as u8,
    ];

    for py in 0..size {
        for px in 0..size {
            let mut acc = [0f32; 3];
            let mut coverage = 0f32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let Some(layer) = sample(x, y) else {
                        continue;
                    };
                    let rgb = match layer {
                        Layer::Body => color,
                        Layer::Lens => dark,
                        Layer::Strap => strap,
                    };
                    acc[0] += rgb[0] as f32;
                    acc[1] += rgb[1] as f32;
                    acc[2] += rgb[2] as f32;
                    coverage += 1.0;
                }
            }
            let index = ((py * size + px) * 4) as usize;
            if coverage > 0.0 {
                let alpha = coverage / (SS * SS) as f32;
                out[index] = (acc[0] / coverage) as u8;
                out[index + 1] = (acc[1] / coverage) as u8;
                out[index + 2] = (acc[2] / coverage) as u8;
                out[index + 3] = (alpha * 255.0) as u8;
            }
        }
    }
    out
}

/// Convert RGBA8 to the ARGB32 big-endian layout the StatusNotifierItem spec
/// wants.
pub fn rgba_to_argb32(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        out.push(pixel[3]);
        out.push(pixel[0]);
        out.push(pixel[1]);
        out.push(pixel[2]);
    }
    out
}

/// Tray icon pixmaps at the sizes panels usually ask for.
pub fn tray_pixmaps(state: IconState) -> Vec<ksni::Icon> {
    [22u32, 32, 48, 64]
        .into_iter()
        .map(|size| {
            let rgba = render_rgba(size, state.color());
            ksni::Icon {
                width: size as i32,
                height: size as i32,
                data: rgba_to_argb32(&rgba),
            }
        })
        .collect()
}

/// Window icon for the GUI.
pub fn window_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    egui::IconData {
        rgba: render_rgba(SIZE, IconState::Ready.color()),
        width: SIZE,
        height: SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_the_right_buffer_size() {
        let rgba = render_rgba(32, [10, 20, 30]);
        assert_eq!(rgba.len(), 32 * 32 * 4);
    }

    #[test]
    fn icon_has_both_opaque_and_transparent_pixels() {
        let rgba = render_rgba(64, [200, 100, 50]);
        let alphas: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3]).collect();
        assert!(alphas.contains(&255), "expected solid pixels");
        assert!(alphas.contains(&0), "expected empty corners");
    }

    #[test]
    fn corners_are_empty_and_the_centre_is_drawn() {
        let size = 64u32;
        let rgba = render_rgba(size, [200, 100, 50]);
        let alpha_at = |x: u32, y: u32| rgba[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha_at(0, 0), 0);
        assert_eq!(alpha_at(size - 1, 0), 0);
        assert_eq!(alpha_at(size / 2, size / 2), 255);
    }

    #[test]
    fn argb_conversion_reorders_channels() {
        let rgba = vec![1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(rgba_to_argb32(&rgba), vec![4, 1, 2, 3, 8, 5, 6, 7]);
    }

    #[test]
    fn tray_pixmaps_cover_common_panel_sizes() {
        let icons = tray_pixmaps(IconState::Connected);
        assert_eq!(icons.len(), 4);
        for icon in &icons {
            assert_eq!(
                icon.data.len(),
                (icon.width * icon.height * 4) as usize,
                "pixmap payload must match its dimensions"
            );
        }
    }

    #[test]
    fn states_have_distinct_colors() {
        let colors = [
            IconState::Idle.color(),
            IconState::Ready.color(),
            IconState::Connected.color(),
            IconState::Problem.color(),
        ];
        for (i, a) in colors.iter().enumerate() {
            for b in colors.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn window_icon_is_square_and_complete() {
        let icon = window_icon();
        assert_eq!(icon.width, icon.height);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
