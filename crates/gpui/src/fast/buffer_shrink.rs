//! When a renderer's instance buffer gives memory back.
//!
//! Instance buffers grow to the biggest frame drawn and are never freed, so
//! one huge frame keeps its buffer for the life of the window. The renderer
//! crates include this file by path, as it is not part of `gpui` itself.

/// Drawn frames in a row that must stay under a quarter of the capacity.
pub(crate) const QUIET_FRAMES: u32 = 120;

/// Watches how much of a buffer frames use.
#[derive(Default)]
pub(crate) struct Shrink {
    quiet_frames: u32,
    peak: usize,
}

impl Shrink {
    /// Records a drawn frame that used `used` of `capacity` and returns the
    /// capacity to shrink to, if the buffer has been mostly idle long enough.
    /// Twice the recent peak, rounded up to a power of two, leaves room for
    /// the frames to vary without growing the buffer again. `initial` is the
    /// floor.
    pub(crate) fn frame(&mut self, used: usize, capacity: usize, initial: usize) -> Option<usize> {
        if capacity <= initial || used.saturating_mul(4) >= capacity {
            *self = Self::default();
            return None;
        }
        self.peak = self.peak.max(used);
        self.quiet_frames += 1;
        if self.quiet_frames < QUIET_FRAMES {
            return None;
        }
        let target = self.peak.saturating_mul(2).next_power_of_two().max(initial);
        *self = Self::default();
        (target < capacity).then_some(target)
    }
}

/// A buffer that is uploaded to any number of times in a frame, as one that
/// holds a draw's batches is. Only the frame's biggest upload counts.
#[allow(
    dead_code,
    reason = "only the DirectX renderer judges buffers uploaded to many times a frame"
)]
pub(crate) struct Use {
    shrink: Shrink,
    peak: usize,
    initial: usize,
}

#[allow(dead_code)]
impl Use {
    pub(crate) fn new(initial: usize) -> Self {
        Self {
            shrink: Shrink::default(),
            peak: 0,
            initial,
        }
    }

    /// Records an upload of `used` of the buffer's capacity.
    pub(crate) fn note(&mut self, used: usize) {
        self.peak = self.peak.max(used);
    }

    /// Ends a frame: the capacity to shrink to, if the buffer has been mostly
    /// idle long enough. A frame that made no upload counts as idle.
    pub(crate) fn end_frame(&mut self, capacity: usize) -> Option<usize> {
        let used = std::mem::take(&mut self.peak);
        self.shrink.frame(used, capacity, self.initial)
    }
}

#[cfg(test)]
mod tests {
    use super::{QUIET_FRAMES, Shrink, Use};

    const INITIAL: usize = 2 << 20;
    const HUGE: usize = 128 << 20;

    fn quiet(shrink: &mut Shrink, used: usize, capacity: usize, frames: u32) -> Option<usize> {
        (0..frames).find_map(|_| shrink.frame(used, capacity, INITIAL))
    }

    #[test]
    fn a_buffer_shrinks_after_a_quiet_stretch_to_twice_the_peak() {
        let mut shrink = Shrink::default();
        assert_eq!(quiet(&mut shrink, 3_000_000, HUGE, QUIET_FRAMES - 1), None);
        assert_eq!(shrink.frame(5_000_000, HUGE, INITIAL), Some(16 << 20));
    }

    #[test]
    fn it_never_shrinks_below_the_initial_size() {
        let mut shrink = Shrink::default();
        assert_eq!(quiet(&mut shrink, 10, HUGE, QUIET_FRAMES), Some(INITIAL));
        assert_eq!(quiet(&mut shrink, 10, INITIAL, 10 * QUIET_FRAMES), None);
    }

    #[test]
    fn a_busy_frame_restarts_the_count() {
        let mut shrink = Shrink::default();
        for _ in 0..3 {
            assert_eq!(quiet(&mut shrink, 1000, HUGE, QUIET_FRAMES - 1), None);
            assert_eq!(shrink.frame(HUGE / 4, HUGE, INITIAL), None);
        }
    }

    #[test]
    fn alternating_big_and_normal_frames_never_shrink() {
        let mut shrink = Shrink::default();
        for _ in 0..50 {
            assert_eq!(quiet(&mut shrink, 1000, HUGE, 60), None);
            assert_eq!(shrink.frame(HUGE / 2, HUGE, INITIAL), None);
        }
    }

    #[test]
    fn many_uploads_in_a_frame_count_as_one_frame_of_the_biggest() {
        let mut buffer = Use::new(INITIAL);
        for frame in 0..QUIET_FRAMES {
            // A big batch among small ones: the frame is busy.
            for batch in 0..20 {
                buffer.note(if batch == 7 { HUGE / 2 } else { 100 });
            }
            assert_eq!(buffer.end_frame(HUGE), None, "frame {frame}");
        }
        for _ in 0..QUIET_FRAMES - 1 {
            for _ in 0..20 {
                buffer.note(100);
            }
            assert_eq!(buffer.end_frame(HUGE), None);
        }
        buffer.note(100);
        assert_eq!(buffer.end_frame(HUGE), Some(INITIAL));
    }
}
