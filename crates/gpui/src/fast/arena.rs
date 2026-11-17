//! How many chunks the element arena keeps between frames.
//!
//! Every element of a frame is allocated in the arena, and its chunks used to
//! stay allocated after the frame was cleared: one frame that built a large
//! tree (mounting thousands of views) kept tens of megabytes for the life of
//! the app, though the frames after it build a few elements each. The arena
//! now keeps the chunks the last two frames needed, up to [`MAX_KEPT_CHUNKS`],
//! and frees the rest: a single small frame between two large ones does not
//! make the next large one allocate again, and an app left idle after a large
//! frame holds a few megabytes rather than all of it.

/// The most chunks kept between frames. A frame needing more allocates them
/// again, which costs little next to building that many elements.
pub(crate) const MAX_KEPT_CHUNKS: usize = 8;

/// Frees every chunk but the first while the app is idle and nothing is
/// allocated in the arena. See [`crate::fast::idle_trim`].
pub(crate) fn release_idle(arena: &mut crate::arena::Arena) {
    if arena.scope_depth != 0 || !arena.elements.is_empty() {
        return;
    }
    arena.chunks.truncate(1);
    arena.chunks.shrink_to_fit();
    arena.elements.shrink_to_fit();
    arena.trim.previous_frame = 1;
}

/// What the arena remembers between clears.
#[derive(Default)]
pub(crate) struct ChunkTrim {
    previous_frame: usize,
}

impl ChunkTrim {
    /// Called as the arena is cleared, after its elements were dropped, with
    /// the index of the last chunk the frame allocated from. Frees the chunks
    /// neither this frame nor the one before it needed.
    pub(crate) fn trim<T>(&mut self, chunks: &mut Vec<T>, last_used: usize) {
        let used = last_used + 1;
        let keep = used.max(self.previous_frame).clamp(1, MAX_KEPT_CHUNKS);
        self.previous_frame = used;
        if chunks.len() > keep {
            chunks.truncate(keep);
            chunks.shrink_to_fit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkTrim;

    #[test]
    fn keeps_what_the_last_two_frames_used() {
        let mut trim = ChunkTrim::default();
        let mut chunks = vec![(); 6];
        trim.trim(&mut chunks, 5);
        assert_eq!(chunks.len(), 6);
        trim.trim(&mut chunks, 0);
        assert_eq!(chunks.len(), 6);
        trim.trim(&mut chunks, 0);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn keeps_no_more_than_the_cap() {
        let mut trim = ChunkTrim::default();
        let mut chunks = vec![(); 48];
        trim.trim(&mut chunks, 47);
        assert_eq!(chunks.len(), super::MAX_KEPT_CHUNKS);
    }

    #[test]
    fn never_frees_the_first_chunk() {
        let mut trim = ChunkTrim::default();
        let mut chunks = vec![(); 1];
        trim.trim(&mut chunks, 0);
        trim.trim(&mut chunks, 0);
        assert_eq!(chunks.len(), 1);
    }
}
