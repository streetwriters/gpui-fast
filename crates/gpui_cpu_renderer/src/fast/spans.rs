//! Writing the interior of an opaque, one-colour primitive a row at a time.
//!
//! A pixel of such a primitive is tested against the primitive's bounds and
//! its content mask and, if it is inside them, gets the same four bytes as
//! every other: the colour has an alpha of one, so what it is drawn over does
//! not show. The pixels that pass the tests are a run of each row, and this
//! finds the run and leaves the shader and the blend out.

/// The columns of `x0..x1` that `inside` is true for, as `start..end`, given
/// that they are an unbroken run (`inside` is a pair of comparisons with a
/// column's centre) and that `estimate` is where the run begins and ends to
/// within a pixel.
///
/// The estimate is corrected against `inside` itself, so the answer is exactly
/// the columns the per-pixel tests accept. `None` when it cannot be known to
/// be that: the run is empty or the estimate was off by more than a pixel; the
/// row is then drawn pixel by pixel.
pub(crate) fn columns(
    (x0, x1): (i32, i32),
    (estimate_start, estimate_end): (f32, f32),
    inside: impl Fn(i32) -> bool,
) -> Option<(i32, i32)> {
    if !(estimate_start.is_finite() && estimate_end.is_finite()) {
        return None;
    }
    let clamp = |estimate: f32| estimate.clamp(x0 as f32, x1 as f32) as i32;
    let (mut start, mut end) = (clamp(estimate_start), clamp(estimate_end));
    if start > x0 && inside(start - 1) {
        start -= 1;
    }
    if start < x1 && !inside(start) {
        start += 1;
    }
    if end < x1 && inside(end) {
        end += 1;
    }
    if end > x0 && !inside(end - 1) {
        end -= 1;
    }
    let ends_are_the_run = start < end
        && inside(start)
        && inside(end - 1)
        && (start == x0 || !inside(start - 1))
        && (end == x1 || !inside(end));
    ends_are_the_run.then_some((start, end))
}

/// Writes `bytes` to each of the pixels of `row` from `start` to `end`, the
/// row beginning at column `x0`.
pub(crate) fn fill(row: &mut [u8], x0: i32, (start, end): (i32, i32), bytes: [u8; 4]) {
    let range = (start - x0) as usize * 4..(end - x0) as usize * 4;
    for pixel in row[range].chunks_exact_mut(4) {
        pixel.copy_from_slice(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::{columns, fill};

    /// Columns whose centres are in `[low, high)`, as the renderer's per-pixel
    /// test has it.
    fn inside(low: f32, high: f32) -> impl Fn(i32) -> bool {
        move |x| {
            let centre = x as f32 + 0.5;
            !(centre < low || centre >= high)
        }
    }

    fn brute_force(range: (i32, i32), test: impl Fn(i32) -> bool) -> Option<(i32, i32)> {
        let found: Vec<i32> = (range.0..range.1).filter(|&x| test(x)).collect();
        Some((*found.first()?, *found.last()? + 1))
    }

    /// A deterministic stream of numbers in `[0, 1)`.
    fn random() -> impl FnMut() -> f32 {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    #[test]
    fn the_run_is_the_columns_the_test_accepts_whatever_the_edges_are() {
        let mut next = random();
        for _ in 0..20_000 {
            let range = (-3, 40 + (next() * 400.) as i32);
            let low = -20. + next() * 300.;
            let high = low + next() * 300.;
            let test = inside(low, high);
            let estimate = ((low - 0.5).ceil(), (high - 0.5).ceil());
            let expected = brute_force(range, &test);
            match columns(range, estimate, &test) {
                Some(found) => assert_eq!(Some(found), expected, "{low}..{high}"),
                // Giving up is always allowed, but not when there is a run
                // and the estimate is the exact one.
                None => assert_eq!(expected, None, "{low}..{high} was given up on"),
            }
        }
    }

    #[test]
    fn an_estimate_a_pixel_off_either_way_is_corrected() {
        let test = inside(10.2, 20.7);
        let range = (0, 40);
        let exact = brute_force(range, &test);
        for start in [-1., 0., 1.] {
            for end in [-1., 0., 1.] {
                let estimate = ((10.2f32 - 0.5).ceil() + start, (20.7f32 - 0.5).ceil() + end);
                assert_eq!(columns(range, estimate, &test), exact, "{start} {end}");
            }
        }
    }

    #[test]
    fn an_estimate_far_off_or_not_a_number_is_given_up_on() {
        let test = inside(10.2, 20.7);
        assert_eq!(columns((0, 40), (3., 30.), &test), None);
        assert_eq!(columns((0, 40), (f32::NAN, 30.), &test), None);
        assert_eq!(columns((0, 40), (3., f32::INFINITY), &test), None);
    }

    #[test]
    fn an_empty_run_is_given_up_on() {
        assert_eq!(columns((0, 40), (50., 60.), inside(50., 60.)), None);
        assert_eq!(columns((0, 40), (5., 5.), inside(5.2, 5.3)), None);
    }

    #[test]
    fn a_span_only_touches_its_pixels() {
        let mut row = vec![0u8; 10 * 4];
        fill(&mut row, 5, (7, 10), [1, 2, 3, 4]);
        for (index, pixel) in row.chunks_exact(4).enumerate() {
            let column = 5 + index as i32;
            let drawn = (7..10).contains(&column);
            assert_eq!(pixel == [1, 2, 3, 4], drawn, "column {column}");
            assert!(drawn || pixel == [0; 4]);
        }
    }
}
