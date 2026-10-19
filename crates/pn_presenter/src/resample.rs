//! The presenter's own scaler (session 15): GDI's `HALFTONE` is not rect-invariant: the
//! same scale smooths some sub-rect sizes and not others (measured: a 60x20 blit smoothed,
//! a 64x48 or 200x24 one came out nearest), so a dirty-rect present of new text did not
//! match the full-frame present around it. Here every output pixel is a function of its
//! own position only, so any dirty rect reproduces exactly what a full frame would show.

// Pixel index math between usize, i32 and f64 is the whole job.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

/// One axis: for each output coordinate, the two source taps and the weight (0..=256) of
/// the second. Pixel centres are aligned (`(d + 0.5) * s - 0.5`); nearest takes one tap.
fn taps(
    src_len: usize,
    dst_len: usize,
    from: usize,
    to: usize,
    smooth: bool,
) -> Vec<(usize, usize, u32)> {
    let s = src_len as f64 / dst_len as f64;
    let last = src_len - 1;
    (from..to)
        .map(|d| {
            if !smooth {
                let i = (((d as f64 + 0.5) * s) as usize).min(last);
                return (i, i, 0);
            }
            let f = ((d as f64 + 0.5) * s - 0.5).max(0.0);
            let i = (f as usize).min(last);
            let w = ((f - i as f64) * 256.0).round() as u32;
            (i, (i + 1).min(last), w.min(256))
        })
        .collect()
}

/// `a` and `b` mixed with `w`/256 of `b`, per 8-bit channel (the 4th byte is dropped).
fn lerp(lo: u32, hi: u32, weight: u32) -> u32 {
    let keep = 256 - weight;
    let rb = (((lo & 0x00ff_00ff) * keep + (hi & 0x00ff_00ff) * weight) >> 8) & 0x00ff_00ff;
    let g = (((lo & 0x0000_ff00) * keep + (hi & 0x0000_ff00) * weight) >> 8) & 0x0000_ff00;
    rb | g
}

/// Writes `rect` (`[x, y, w, h]`, inside `dst`) of the `dw x dh` scaled image of the
/// `sw x sh` source. Both images are top-down rows of 32-bit pixels.
pub fn resample(
    src: &[u32],
    (sw, sh): (usize, usize),
    dst: &mut [u32],
    (dw, dh): (usize, usize),
    rect: [usize; 4],
    smooth: bool,
) {
    let [x, y, w, h] = rect;
    if sw == 0 || sh == 0 || src.len() < sw * sh || dst.len() < dw * dh || x + w > dw || y + h > dh
    {
        return;
    }
    let cols = taps(sw, dw, x, x + w, smooth);
    for (row, &(y0, y1, wy)) in (y..y + h).zip(taps(sh, dh, y, y + h, smooth).iter()) {
        let (r0, r1) = (&src[y0 * sw..(y0 + 1) * sw], &src[y1 * sw..(y1 + 1) * sw]);
        let out = &mut dst[row * dw + x..row * dw + x + w];
        for (o, &(x0, x1, wx)) in out.iter_mut().zip(&cols) {
            *o = lerp(lerp(r0[x0], r0[x1], wx), lerp(r1[x0], r1[x1], wx), wy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: usize, h: usize) -> Vec<u32> {
        (0..w * h)
            .map(|i| {
                if (i % w + i / w).is_multiple_of(2) {
                    0
                } else {
                    0x00ff_ffff
                }
            })
            .collect()
    }

    /// The property GDI lacked: a dirty rect is pixel-identical to the same area of a
    /// full-frame present, for every rect size, smooth or nearest.
    #[test]
    fn any_dirty_rect_matches_the_full_frame() {
        let (sw, sh, dw, dh) = (64, 48, 139, 104); // 2.17x, the user's fit scale
        let src = checker(sw, sh);
        for smooth in [true, false] {
            let mut full = vec![0u32; dw * dh];
            resample(&src, (sw, sh), &mut full, (dw, dh), [0, 0, dw, dh], smooth);
            for rect in [
                [3, 5, 40, 20],
                [50, 70, 7, 9],
                [0, 0, 1, 1],
                [100, 90, 39, 14],
            ] {
                let mut part = vec![0u32; dw * dh];
                resample(&src, (sw, sh), &mut part, (dw, dh), rect, smooth);
                for yy in rect[1]..rect[1] + rect[3] {
                    for xx in rect[0]..rect[0] + rect[2] {
                        assert_eq!(part[yy * dw + xx], full[yy * dw + xx], "{rect:?} {smooth}");
                    }
                }
            }
        }
    }

    #[test]
    fn flat_stays_flat_and_nearest_replicates() {
        let src = vec![0x0012_3456u32; 4 * 4];
        let mut dst = vec![0u32; 9 * 9];
        resample(&src, (4, 4), &mut dst, (9, 9), [0, 0, 9, 9], true);
        assert!(dst.iter().all(|&p| p == 0x0012_3456));
        let src = checker(2, 2);
        let mut dst = vec![0u32; 16];
        resample(&src, (2, 2), &mut dst, (4, 4), [0, 0, 4, 4], false);
        assert_eq!(&dst[..4], &[0, 0, 0x00ff_ffff, 0x00ff_ffff]);
    }

    #[test]
    fn smooth_blends_between_neighbours() {
        let src = vec![0u32, 0x00ff_ffff];
        let mut dst = vec![0u32; 4];
        resample(&src, (2, 1), &mut dst, (4, 1), [0, 0, 4, 1], true);
        let g = |p: u32| p & 0xff;
        assert_eq!(g(dst[0]), 0);
        assert!(g(dst[1]) > 40 && g(dst[1]) < 90 && g(dst[2]) > 165 && g(dst[2]) < 215);
        assert_eq!(g(dst[3]), 0xff);
    }
}
