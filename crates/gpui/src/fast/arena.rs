//! How many chunks the element arena keeps between frames.
//!
//! Every element of a frame is allocated in the arena, and its chunks used to
//! stay allocated after the frame was cleared: one frame that built a large
//! tree (mounting thousands of views) kept tens of megabytes for the life of
//! the app, though the frames after it build a few elements each. The arena
//! now keeps the chunks the last two frames needed and frees the rest, so a
//! single small frame between two large ones does not make the next large one
//! allocate again.

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
        let keep = used.max(self.previous_frame).max(1);
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
        let mut chunks = vec![(); 48];
        trim.trim(&mut chunks, 47);
        assert_eq!(chunks.len(), 48);
        trim.trim(&mut chunks, 0);
        assert_eq!(chunks.len(), 48);
        trim.trim(&mut chunks, 0);
        assert_eq!(chunks.len(), 1);
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
