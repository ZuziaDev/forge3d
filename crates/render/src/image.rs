//! RGBA8 images and PNG encoding.

use crate::error::{RenderError, RenderResult};

/// An 8-bit sRGB RGBA image, rows top to bottom, straight (non-premultiplied) alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// Image filled with one color.
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> RgbaImage {
        let n = width as usize * height as usize;
        let mut pixels = Vec::with_capacity(n * 4);
        for _ in 0..n {
            pixels.extend_from_slice(&fill);
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// Pixel at (`x`, `y`); panics when out of range.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "pixel out of range");
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Encode as PNG.
    pub fn to_png(&self) -> RenderResult<Vec<u8>> {
        encode_png(self.width, self.height, &self.pixels)
    }
}

/// Encode RGBA8 pixels as a PNG file in memory.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> RenderResult<Vec<u8>> {
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(RenderError::Png(format!(
            "pixel buffer has {} bytes, expected {}",
            rgba.len(),
            width as usize * height as usize * 4
        )));
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc
            .write_header()
            .map_err(|e| RenderError::Png(e.to_string()))?;
        w.write_image_data(rgba)
            .map_err(|e| RenderError::Png(e.to_string()))?;
        w.finish().map_err(|e| RenderError::Png(e.to_string()))?;
    }
    Ok(out)
}
