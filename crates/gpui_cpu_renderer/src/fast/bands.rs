//! Rasterizing a primitive across threads, a band of rows to each.
//!
//! The per-pixel loop of a primitive reads the scene and the pixel it writes
//! and nothing else, so its rows can be drawn in any order, on any thread,
//! and the frame is the same bytes as when they are drawn one after the other.
//! A primitive too small to pay for waking the pool is drawn on the calling
//! thread.

use rayon::prelude::*;

use crate::Canvas;

/// A primitive of fewer pixels than this is drawn on the calling thread: the
/// glyphs and thin borders of an interface are, and waking the pool costs more
/// than they do.
const MIN_PARALLEL_PIXELS: usize = 16 * 1024;

/// The fewest rows in a band.
const MIN_BAND_ROWS: usize = 4;

/// Bands per thread, so that a primitive whose cost varies by row (a shadow's
/// falloff, a gradient's clipped corners) still keeps every thread busy.
const BANDS_PER_THREAD: usize = 4;

/// Calls `row` for each row `y` in `y0..y1` of the canvas with the pixels of
/// `x0..x1` in it, four bytes each. The ranges are inside the canvas.
pub(crate) fn for_each_row(
    canvas: &mut Canvas,
    (x0, x1): (i32, i32),
    (y0, y1): (i32, i32),
    row: impl Fn(i32, &mut [u8]) + Sync,
) {
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let stride = canvas.width as usize * 4;
    let rows = (y1 - y0) as usize;
    let columns = (x1 - x0) as usize;
    let first = (y0 - canvas.top) as usize;
    let skip = (x0 - canvas.left) as usize * 4;
    let data = &mut canvas.pixmap.data_mut()[first * stride..(first + rows) * stride];
    let threads = rayon::current_num_threads();
    if threads <= 1 || rows < 2 * MIN_BAND_ROWS || rows * columns < MIN_PARALLEL_PIXELS {
        for (index, row_data) in data.chunks_exact_mut(stride).enumerate() {
            row(y0 + index as i32, &mut row_data[skip..skip + columns * 4]);
        }
        return;
    }
    let band_rows = rows.div_ceil(threads * BANDS_PER_THREAD).max(MIN_BAND_ROWS);
    data.par_chunks_mut(band_rows * stride)
        .enumerate()
        .for_each(|(band, band_data)| {
            let top = y0 + (band * band_rows) as i32;
            for (index, row_data) in band_data.chunks_exact_mut(stride).enumerate() {
                row(top + index as i32, &mut row_data[skip..skip + columns * 4]);
            }
        });
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use super::for_each_row;
    use crate::Canvas;

    fn pool(threads: usize) -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("a pool")
    }

    /// Draws `rows` of a 256 pixel wide canvas, noting each row drawn and
    /// writing the row's number into its pixels.
    fn draw(threads: usize, rows: i32) -> (Canvas, Vec<i32>) {
        let mut canvas = Canvas::crop(0, 0, 256, rows as u32).unwrap();
        let seen = Mutex::new(Vec::new());
        pool(threads).install(|| {
            for_each_row(&mut canvas, (0, 256), (0, rows), |y, row| {
                seen.lock().unwrap().push(y);
                assert_eq!(row.len(), 256 * 4);
                row.fill(y as u8);
            });
        });
        let mut seen = seen.into_inner().unwrap();
        seen.sort();
        (canvas, seen)
    }

    #[test]
    fn a_large_primitive_is_split_into_bands_across_threads() {
        // Rows are slow enough that the pool's other threads get to take bands.
        let mut canvas = Canvas::crop(0, 0, 256, 256).unwrap();
        let drawn_on = Mutex::new(HashSet::new());
        pool(4).install(|| {
            for_each_row(&mut canvas, (0, 256), (0, 256), |_, _| {
                drawn_on.lock().unwrap().insert(std::thread::current().id());
                std::thread::sleep(std::time::Duration::from_micros(200));
            });
        });
        assert!(
            drawn_on.into_inner().unwrap().len() > 1,
            "a 256x256 primitive was drawn on one thread"
        );
    }

    #[test]
    fn every_row_is_drawn_once_with_its_own_number() {
        for threads in [1, 4] {
            let (mut canvas, seen) = draw(threads, 200);
            assert_eq!(seen, (0..200).collect::<Vec<_>>(), "{threads} threads");
            let stride = 256 * 4;
            for (y, row) in canvas.pixmap.data_mut().chunks_exact(stride).enumerate() {
                assert!(row.iter().all(|&byte| byte == y as u8), "row {y}");
            }
        }
    }

    #[test]
    fn a_small_primitive_stays_on_the_calling_thread() {
        let mut canvas = Canvas::crop(0, 0, 64, 64).unwrap();
        pool(4).install(|| {
            let caller = std::thread::current().id();
            for_each_row(&mut canvas, (0, 64), (0, 64), |_, _| {
                assert_eq!(std::thread::current().id(), caller);
            });
        });
    }

    #[test]
    fn a_range_inside_the_canvas_only_touches_its_pixels() {
        let mut canvas = Canvas::crop(10, 20, 300, 300).unwrap();
        pool(4).install(|| {
            for_each_row(&mut canvas, (50, 250), (60, 290), |_, row| {
                assert_eq!(row.len(), 200 * 4);
                row.fill(7);
            });
        });
        let stride = 300 * 4;
        for (y, row) in canvas.pixmap.data_mut().chunks_exact(stride).enumerate() {
            for (x, pixel) in row.chunks_exact(4).enumerate() {
                let inside = (40..240).contains(&x) && (40..270).contains(&y);
                assert_eq!(pixel == [7; 4], inside, "pixel ({x}, {y})");
            }
        }
    }
}
