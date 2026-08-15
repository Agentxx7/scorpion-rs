#![cfg(feature = "local_qwen3_vl")]

//! Phase-1 (input parity) diagnostic for
//! `SCORPION_QWEN3_VL_CANDLE_REFERENCE_PARITY_ROOT_CAUSE_001`: print the
//! same preprocessing statistics (shape, min/max/mean/std) the Python
//! reference oracle prints, for the identical fixtures, so they can be
//! diffed by hand. Not a regression gate.

use candle::{Device, Tensor};
use image::{DynamicImage, ImageBuffer, Rgb};
use std::io::Cursor;

fn gradient_fixture() -> Vec<u8> {
    let image = ImageBuffer::from_fn(96, 64, |x, y| {
        Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
    });
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

fn square_fixture(width: u32, height: u32) -> Vec<u8> {
    let side = (width.min(height) / 14).max(16);
    let mut canvas = ImageBuffer::from_pixel(width, height, Rgb([40u8, 40, 40]));
    let half = side / 2;
    let cx = side;
    let cy = side;
    for y in cy.saturating_sub(half)..(cy + half).min(height) {
        for x in cx.saturating_sub(half)..(cx + half).min(width) {
            canvas.put_pixel(x, y, Rgb([220, 40, 40]));
        }
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(canvas)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

fn stats(tensor: &Tensor) -> (f64, f64, f64, f64) {
    let values: Vec<f32> = tensor.flatten_all().unwrap().to_vec1().unwrap();
    let min = values.iter().cloned().fold(f32::INFINITY, f32::min) as f64;
    let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
    let mean = values.iter().map(|v| *v as f64).sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|v| (*v as f64 - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    (min, max, mean, variance.sqrt())
}

#[test]
fn print_preprocess_stats() {
    let device = Device::Cpu;
    for (label, bytes) in [
        ("gradient_96x64", gradient_fixture()),
        ("square_320x224", square_fixture(320, 224)),
    ] {
        let processed = spider::features::qwen3_vl_runtime::process_image(&bytes, &device)
            .expect("process_image must succeed");
        let (min, max, mean, std) = stats(&processed.pixel_values);
        eprintln!(
            "[{label}] original={:?} processed={:?} grid={:?} pixel_values.dims={:?} \
             min={min:.4} max={max:.4} mean={mean:.4} std={std:.4} merged_tokens={}",
            processed.original_dimensions,
            processed.processed_dimensions,
            processed.image_grid_thw,
            processed.pixel_values.dims(),
            processed.merged_visual_tokens,
        );
    }
}
