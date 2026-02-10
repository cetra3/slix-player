use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub const BAR_WIDTH: u32 = 3;
pub const BAR_GAP: u32 = 1;
pub const BAR_STEP: u32 = BAR_WIDTH + BAR_GAP;
pub const WAVEFORM_HEIGHT: u32 = 80;

/// Downsample high-resolution peaks to a fixed number of display bins.
/// Returns normalized values (0.0–1.0).
pub fn compute_bins(peaks: &[f32], num_bins: usize) -> Vec<f32> {
    if peaks.is_empty() || num_bins == 0 {
        return vec![0.0; num_bins];
    }

    let peaks_per_bin = peaks.len() / num_bins;
    if peaks_per_bin == 0 {
        let mut bins = peaks.to_vec();
        bins.resize(num_bins, 0.0);
        return normalize(bins);
    }

    let mut bins = Vec::with_capacity(num_bins);
    for i in 0..num_bins {
        let start = i * peaks_per_bin;
        let end = ((i + 1) * peaks_per_bin).min(peaks.len());
        let peak = peaks[start..end]
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        bins.push(peak);
    }

    normalize(bins)
}

fn normalize(mut bins: Vec<f32>) -> Vec<f32> {
    let max = bins.iter().cloned().fold(0.0f32, f32::max);
    if max > 0.0 {
        for b in &mut bins {
            *b /= max;
        }
    }
    bins
}

/// Set a pixel in the RGBA buffer.
fn set_pixel(bytes: &mut [u8], width: u32, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
    let idx = ((y * width + x) * 4) as usize;
    if idx + 3 < bytes.len() {
        bytes[idx] = r;
        bytes[idx + 1] = g;
        bytes[idx + 2] = b;
        bytes[idx + 3] = a;
    }
}

/// Render waveform bins to raw RGBA bytes (Send-safe, no Slint types).
/// Peak-max bars drawn dim behind brighter RMS bars.
/// If `bins_max` is empty, only RMS is drawn.
fn render_waveform_rgba(
    bins_rms: &[f32],
    bins_max: &[f32],
    width: u32,
    height: u32,
    bar_w: u32,
    bar_step: u32,
) -> Vec<u8> {
    let len = (width * height * 4) as usize;
    let mut bytes = vec![0u8; len];

    let center_y = height / 2;
    let half_h = center_y.saturating_sub(1);
    let has_max = !bins_max.is_empty();

    for (i, &rms_amp) in bins_rms.iter().enumerate() {
        let x_start = i as u32 * bar_step;
        if x_start + bar_w > width {
            break;
        }

        let max_amp = if has_max {
            bins_max.get(i).copied().unwrap_or(rms_amp)
        } else {
            rms_amp
        };

        let bar_h_rms = if rms_amp > 0.01 {
            ((rms_amp * half_h as f32) as u32).max(1)
        } else {
            0
        };
        let bar_h_max = if max_amp > 0.01 {
            ((max_amp * half_h as f32) as u32).max(1)
        } else {
            0
        };

        let outer = bar_h_max.max(bar_h_rms);
        for dy in 0..outer {
            let (r, g, b, a) = if dy < bar_h_rms {
                (255, 255, 255, 255) // RMS: full white
            } else {
                (255, 255, 255, 90) // Peak-max: dim
            };

            let y_up = center_y.saturating_sub(1 + dy);
            if y_up < height {
                for dx in 0..bar_w {
                    set_pixel(&mut bytes, width, x_start + dx, y_up, r, g, b, a);
                }
            }

            let y_down = center_y + dy;
            if y_down < height {
                for dx in 0..bar_w {
                    set_pixel(&mut bytes, width, x_start + dx, y_down, r, g, b, a);
                }
            }
        }
    }

    bytes
}

/// Reconstruct a Slint Image from raw RGBA bytes.
pub fn image_from_rgba(rgba: &[u8], width: u32, height: u32) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height);
    Image::from_rgba8(buffer)
}

/// Render a cover art image with a solid radial waveform blob.
/// Filled from center to an outer edge shaped by peaks, 4x SSAA antialiasing.
/// `scale` multiplies the base 140x140 size for HiDPI displays.
pub fn render_cover_art(peaks: &[f32], peaks_max: &[f32], scale: f32) -> Image {
    let size = (140.0 * scale).round() as u32;
    let center = size as f32 / 2.0;
    let inner_r = 10.0 * scale;
    let max_r = 56.0 * scale;
    const NUM_BINS: usize = 360;
    const AA: u32 = 4; // 4x4 subpixel grid

    let bins = compute_bins(peaks, NUM_BINS);
    let bins_m = if peaks_max.is_empty() {
        vec![]
    } else {
        compute_bins(peaks_max, NUM_BINS)
    };
    let has_max = !bins_m.is_empty();

    // Compute outer radius at each angle bin
    let mut radii_rms = vec![0.0f32; NUM_BINS];
    let mut radii_max = vec![0.0f32; NUM_BINS];
    for i in 0..NUM_BINS {
        let amp = bins[i];
        let max_amp = if has_max {
            bins_m.get(i).copied().unwrap_or(amp)
        } else {
            amp
        };
        radii_rms[i] = inner_r + amp * max_r;
        radii_max[i] = inner_r + max_amp * max_r;
    }

    let mut bytes = vec![0u8; (size * size * 4) as usize];
    let sub = 1.0 / AA as f32;

    for py in 0..size {
        for px in 0..size {
            let mut hits_rms = 0u32;
            let mut hits_max = 0u32;

            for sy in 0..AA {
                for sx in 0..AA {
                    let dx = px as f32 - center + (sx as f32 + 0.5) * sub;
                    let dy = py as f32 - center + (sy as f32 + 0.5) * sub;
                    let dist = (dx * dx + dy * dy).sqrt();

                    if dist < inner_r || dist > inner_r + max_r + 1.0 {
                        continue;
                    }

                    let angle = (dy.atan2(dx) / std::f32::consts::TAU + 0.75).fract();
                    let bin_f = angle * NUM_BINS as f32;
                    let bin0 = bin_f as usize % NUM_BINS;
                    let bin1 = (bin0 + 1) % NUM_BINS;
                    let t = bin_f.fract();

                    let outer_rms = radii_rms[bin0] * (1.0 - t) + radii_rms[bin1] * t;
                    let outer_max = radii_max[bin0] * (1.0 - t) + radii_max[bin1] * t;

                    if dist <= outer_rms {
                        hits_rms += 1;
                    } else if dist <= outer_max {
                        hits_max += 1;
                    }
                }
            }

            let total = AA * AA;
            if hits_rms + hits_max == 0 {
                continue;
            }

            let a_rms = hits_rms as f32 / total as f32 * 255.0;
            let a_max = hits_max as f32 / total as f32 * 90.0;
            let a = (a_rms + a_max).min(255.0) as u8;

            set_pixel(&mut bytes, size, px, py, 0x73, 0xc6, 0xec, a);
        }
    }

    image_from_rgba(&bytes, size, size)
}

/// Render peaks to a Slint Image at a given logical display width.
/// `scale` multiplies pixel dimensions for HiDPI displays.
/// The image width is snapped to a bar-step multiple for pixel-perfect bars.
pub fn render_waveform(peaks_rms: &[f32], peaks_max: &[f32], display_width: u32, height: u32, scale: f32) -> Image {
    let num_bins = (display_width / BAR_STEP) as usize;
    if num_bins == 0 {
        return Image::default();
    }
    let bar_w = (BAR_WIDTH as f32 * scale).round() as u32;
    let bar_step = bar_w + (BAR_GAP as f32 * scale).round() as u32;
    let width = num_bins as u32 * bar_step;
    let h = (height as f32 * scale).round() as u32;
    let bins_rms = compute_bins(peaks_rms, num_bins);
    let bins_max = if peaks_max.is_empty() {
        vec![]
    } else {
        compute_bins(peaks_max, num_bins)
    };
    let rgba = render_waveform_rgba(&bins_rms, &bins_max, width, h, bar_w, bar_step);
    image_from_rgba(&rgba, width, h)
}
