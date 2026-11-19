//! Pictures for a thermal printer: dithering and QR codes → ESC/POS raster bytes.

use image::{imageops::FilterType, DynamicImage};

pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    /// Row-major, true = black dot.
    pub dots: Vec<bool>,
}

/// Resize to `width` dots, brighten by `brightness` (-1..1) and dither (Floyd-Steinberg).
pub fn dither(img: &DynamicImage, width: u32, brightness: f32, smooth: bool) -> Bitmap {
    let width = width.max(8);
    let rgba = img.to_rgba8();
    let h = ((rgba.height() as f32) * width as f32 / rgba.width().max(1) as f32).round().max(1.0) as u32;
    let small = image::imageops::resize(&rgba, width, h, FilterType::Lanczos3);
    let (w, h) = (small.width() as usize, small.height() as usize);

    // luminance 0..255 on a white background
    let mut lum: Vec<f32> = small
        .pixels()
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            (l * a + 255.0 * (1.0 - a) + brightness * 255.0).clamp(0.0, 255.0)
        })
        .collect();

    let mut dots = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let old = lum[i];
            let new = if old < 128.0 { 0.0 } else { 255.0 };
            dots[i] = new == 0.0;
            if smooth {
                let err = old - new;
                if x + 1 < w {
                    lum[i + 1] += err * 7.0 / 16.0;
                }
                if y + 1 < h {
                    if x > 0 {
                        lum[i + w - 1] += err * 3.0 / 16.0;
                    }
                    lum[i + w] += err * 5.0 / 16.0;
                    if x + 1 < w {
                        lum[i + w + 1] += err * 1.0 / 16.0;
                    }
                }
            }
        }
    }
    Bitmap { w, h, dots }
}

pub fn from_bytes(bytes: &[u8], width: u32, brightness: f32, smooth: bool) -> Result<Bitmap, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("can't read that image: {e}"))?;
    Ok(dither(&img, width, brightness, smooth))
}

pub fn from_file(path: &str, width: u32, brightness: f32, smooth: bool) -> Result<Bitmap, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("can't open {path}: {e}"))?;
    from_bytes(&bytes, width, brightness, smooth)
}

/// A QR code with a quiet zone, each module `scale` dots wide.
pub fn qr(text: &str, scale: usize) -> Result<Bitmap, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| format!("can't make a QR code: {e}"))?;
    let n = code.width();
    let colors = code.to_colors();
    let quiet = 2;
    let side = (n + quiet * 2) * scale;
    let mut dots = vec![false; side * side];
    for my in 0..n {
        for mx in 0..n {
            if colors[my * n + mx] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        dots[((my + quiet) * scale + dy) * side + (mx + quiet) * scale + dx] = true;
                    }
                }
            }
        }
    }
    Ok(Bitmap { w: side, h: side, dots })
}

/// ESC/POS `GS v 0` raster print, sent in strips so long images don't overflow the printer's buffer.
pub fn escpos(bm: &Bitmap) -> Vec<u8> {
    let bytes_per_row = (bm.w + 7) / 8;
    let mut out = Vec::new();
    let mut y = 0;
    while y < bm.h {
        let rows = (bm.h - y).min(200);
        out.extend_from_slice(&[0x1d, 0x76, 0x30, 0]);
        out.extend_from_slice(&[
            (bytes_per_row & 0xff) as u8,
            (bytes_per_row >> 8) as u8,
            (rows & 0xff) as u8,
            (rows >> 8) as u8,
        ]);
        for r in y..y + rows {
            for bx in 0..bytes_per_row {
                let mut b = 0u8;
                for bit in 0..8 {
                    let x = bx * 8 + bit;
                    if x < bm.w && bm.dots[r * bm.w + x] {
                        b |= 0x80 >> bit;
                    }
                }
                out.push(b);
            }
        }
        y += rows;
    }
    out
}
