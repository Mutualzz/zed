use crate::{Bounds, DevicePixels, Size, point, size};

/// Padding added on every side of a blurred glyph mask, in device pixels.
///
/// `blur` is a CSS blur radius (`text-shadow` / `drop-shadow`), whose Gaussian has a
/// standard deviation of half the radius; three standard deviations cover the visible
/// part of the blur.
pub(crate) fn blur_padding(blur: u16) -> i32 {
    (f32::from(blur) * 1.5).ceil() as i32
}

/// Grows glyph raster bounds so that the blurred mask fits.
pub(crate) fn blurred_bounds(bounds: Bounds<DevicePixels>, blur: u16) -> Bounds<DevicePixels> {
    if bounds.size.width.0 <= 0 || bounds.size.height.0 <= 0 {
        return bounds;
    }
    let padding = blur_padding(blur);
    Bounds {
        origin: point(
            DevicePixels(bounds.origin.x.0 - padding),
            DevicePixels(bounds.origin.y.0 - padding),
        ),
        size: size(
            DevicePixels(bounds.size.width.0 + padding * 2),
            DevicePixels(bounds.size.height.0 + padding * 2),
        ),
    }
}

fn gaussian_kernel(blur: u16) -> Vec<f32> {
    let sigma = (f32::from(blur) / 2.0).max(f32::EPSILON);
    let radius = blur_padding(blur);
    let mut kernel: Vec<f32> = (-radius..=radius)
        .map(|offset| (-(offset * offset) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = kernel.iter().sum();
    for weight in &mut kernel {
        *weight /= total;
    }
    kernel
}

/// Blurs a one-byte-per-pixel glyph mask with a Gaussian of the given CSS blur radius.
///
/// The returned mask is `blur_padding(blur)` pixels larger on every side, matching
/// [`blurred_bounds`].
pub(crate) fn blur_alpha_mask(
    mask_size: Size<DevicePixels>,
    mask: &[u8],
    blur: u16,
) -> (Size<DevicePixels>, Vec<u8>) {
    let width = mask_size.width.0.max(0) as usize;
    let height = mask_size.height.0.max(0) as usize;
    if width == 0 || height == 0 || mask.len() < width * height {
        return (mask_size, mask.to_vec());
    }
    let padding = blur_padding(blur) as usize;
    let padded_width = width + padding * 2;
    let padded_height = height + padding * 2;
    let mut padded = vec![0.0f32; padded_width * padded_height];
    for row in 0..height {
        for column in 0..width {
            padded[(row + padding) * padded_width + column + padding] =
                f32::from(mask[row * width + column]);
        }
    }
    let kernel = gaussian_kernel(blur);
    let radius = padding as isize;
    let mut horizontal = vec![0.0f32; padded.len()];
    for row in 0..padded_height {
        for column in 0..padded_width {
            let mut sum = 0.0;
            for (index, weight) in kernel.iter().enumerate() {
                let source = column as isize + index as isize - radius;
                if source >= 0 && (source as usize) < padded_width {
                    sum += padded[row * padded_width + source as usize] * weight;
                }
            }
            horizontal[row * padded_width + column] = sum;
        }
    }
    let mut blurred = vec![0u8; padded.len()];
    for row in 0..padded_height {
        for column in 0..padded_width {
            let mut sum = 0.0;
            for (index, weight) in kernel.iter().enumerate() {
                let source = row as isize + index as isize - radius;
                if source >= 0 && (source as usize) < padded_height {
                    sum += horizontal[source as usize * padded_width + column] * weight;
                }
            }
            blurred[row * padded_width + column] = sum.round().clamp(0.0, 255.0) as u8;
        }
    }
    (
        size(
            DevicePixels(padded_width as i32),
            DevicePixels(padded_height as i32),
        ),
        blurred,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blurred_mask_grows_by_the_padding_and_keeps_its_energy() {
        let mask_size = size(DevicePixels(3), DevicePixels(3));
        let mut mask = vec![0u8; 9];
        mask[4] = 255;
        let (blurred_size, blurred) = blur_alpha_mask(mask_size, &mask, 4);
        let padding = blur_padding(4);
        assert_eq!(padding, 6);
        assert_eq!(blurred_size.width.0, 3 + padding * 2);
        assert_eq!(blurred_size.height.0, 3 + padding * 2);
        let total: u32 = blurred.iter().map(|value| u32::from(*value)).sum();
        assert!((200..=310).contains(&total), "{total}");
        let center =
            (blurred_size.width.0 as usize) * (padding as usize + 1) + padding as usize + 1;
        assert!(blurred[center] > blurred[center + 3]);
    }

    #[test]
    fn bounds_grow_on_every_side() {
        let bounds = Bounds {
            origin: point(DevicePixels(2), DevicePixels(-10)),
            size: size(DevicePixels(8), DevicePixels(12)),
        };
        let grown = blurred_bounds(bounds, 2);
        assert_eq!(grown.origin, point(DevicePixels(-1), DevicePixels(-13)));
        assert_eq!(grown.size, size(DevicePixels(14), DevicePixels(18)));
    }
}
