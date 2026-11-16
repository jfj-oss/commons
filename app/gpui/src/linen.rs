//! A very faint linen weave for the cards (design idea #3): a tile of crossing threads in white and black at a few percent,
//! laid over a card's own colour and under its pattern. It is far too quiet to carry meaning, and it never moves.
use gpui_kit::*;
use image::{Frame, RgbaImage};
use smallvec::smallvec;
use std::cell::RefCell;
use std::sync::Arc;

pub const TILE: u32 = 96;

fn hash(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 13;
    (h & 0xFFFF) as f32 / 65535.
}

/// RGBA bytes of the tile (straight alpha is turned into what the atlas wants by the caller). It repeats seamlessly:
/// every period divides the tile size.
pub fn tile_pixels() -> Vec<u8> {
    let mut out = vec![0u8; (TILE * TILE * 4) as usize];
    for y in 0..TILE {
        for x in 0..TILE {
            // warp threads run down (period 4), weft threads run across (period 4), over-under in a plain weave
            let (wx, wy) = (x % 4, y % 4);
            let over = ((x / 4) + (y / 4)) % 2 == 0;
            let warp = if wx == 1 || wx == 2 { 1.0 } else { 0.0 };
            let weft = if wy == 1 || wy == 2 { 1.0 } else { 0.0 };
            let thread = if over { warp } else { weft };
            // a little irregularity, the way real thread is
            let n = hash(x / 2, y / 2) * 0.6 + hash(x, y) * 0.4;
            let light = thread * (0.6 + 0.4 * n);
            let (v, a) = if light > 0.55 { (255., 0.05 * light) } else { (0., 0.04 * (1. - light)) };
            let i = ((y * TILE + x) * 4) as usize;
            // BGRA, premultiplied (what the video and backdrop images are drawn with)
            out[i] = (v * a) as u8; out[i + 1] = (v * a) as u8; out[i + 2] = (v * a) as u8; out[i + 3] = (a * 255.) as u8;
        }
    }
    out
}

thread_local! { static TILE_IMG: RefCell<Option<Arc<RenderImage>>> = const { RefCell::new(None) }; }

pub fn tile() -> Arc<RenderImage> {
    TILE_IMG.with(|t| t.borrow_mut().get_or_insert_with(|| {
        let img = RgbaImage::from_raw(TILE, TILE, tile_pixels()).expect("size");
        Arc::new(RenderImage::new(smallvec![Frame::new(img)]))
    }).clone())
}

/// The element: fills its parent (a card) and lays the weave over it, clipped to the card's rounded corners.
pub fn view(radius: f32) -> impl IntoElement {
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        let t = tile();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let step = TILE as f32;
        let (nx, ny) = ((w / step).ceil() as i32, (h / step).ceil() as i32);
        for j in 0..ny {
            for i in 0..nx {
                let at = Bounds::new(point(bounds.origin.x + px(i as f32 * step), bounds.origin.y + px(j as f32 * step)), size(px(step), px(step)));
                // the rounding belongs to the card's corners only: a tile's own corners stay square
                let r = px(radius);
                let (first_col, last_col, first_row, last_row) = (i == 0, i == nx - 1, j == 0, j == ny - 1);
                let corners = Corners { top_left: if first_col && first_row { r } else { px(0.) }, top_right: if last_col && first_row { r } else { px(0.) },
                    bottom_left: if first_col && last_row { r } else { px(0.) }, bottom_right: if last_col && last_row { r } else { px(0.) } };
                let _ = window.paint_image(bounds, at, corners, t.clone(), 0, false);
            }
        }
    }).size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn the_weave_is_very_faint_and_repeats() {
        let p = tile_pixels();
        assert!(p.chunks(4).all(|c| c[3] <= 14), "alpha stays under 5 percent");
        // the period divides the tile size, so rows and columns wrap without a seam
        assert_eq!(96 % 4, 0);
        assert!(p.chunks(4).any(|c| c[3] > 0));
    }
}
