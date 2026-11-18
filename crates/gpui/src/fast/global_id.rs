//! Global element ids as interned paths, reused across frames by the
//! incremental path hash.
//!
//! An id is the last element id of its path and a link to the id of the path
//! above it, so the ids of a tree share their ancestors: an element costs one
//! node, not a copy of every id from the root down to it.

use crate::{ElementId, GlobalElementId};
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{mem, sync::Arc};

/// The global id of the element id stack of `window`, handed out again from
/// the [`GlobalIdCache`] when it was handed out this frame or the last.
#[inline(always)]
pub(crate) fn current(window: &mut crate::Window) -> GlobalElementId {
    window.global_ids.get(&window.element_id_stack)
}

/// A path of element ids: `None` for the empty one.
#[derive(Clone, Default)]
pub(crate) struct Path(Option<Arc<PathNode>>);

impl std::fmt::Debug for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ids = Vec::new();
        let mut node = &self.0;
        while let Some(current) = node {
            ids.push(&current.id);
            node = &current.parent.path.0;
        }
        ids.reverse();
        f.debug_list().entries(ids).finish()
    }
}

/// The hashes of each prefix of an id stack, as the stack keeps them.
#[inline(always)]
pub(crate) fn prefix_hashes(hashes: &[u64]) -> &[u64] {
    hashes
}

pub(crate) struct PathNode {
    id: ElementId,
    parent: GlobalElementId,
    len: usize,
    /// The ids of the path as a slice, made the first time something asks.
    #[cfg(any(test, feature = "test-support"))]
    flat: std::sync::OnceLock<Box<[ElementId]>>,
}

/// With test support an id reads as the slice of its element ids, root first,
/// as it did before ids were interned: GPUI Kit's test support slices and
/// copies them. Without it nothing builds the slice.
#[cfg(any(test, feature = "test-support"))]
impl std::ops::Deref for GlobalElementId {
    type Target = [ElementId];

    fn deref(&self) -> &[ElementId] {
        match &self.path.0 {
            None => &[],
            Some(node) => node
                .flat
                .get_or_init(|| self.ids().into_iter().cloned().collect()),
        }
    }
}

impl Path {
    /// The id of `ids`, every prefix of it made on the way.
    pub(crate) fn from_ids(ids: &[ElementId]) -> GlobalElementId {
        let mut path = GlobalElementId {
            path: Path(None),
            hash: crate::EMPTY_PATH_HASH,
        };
        for id in ids {
            let hash = crate::extend_path_hash(path.hash, id);
            path = Self::child(path, id.clone(), hash);
        }
        path
    }

    /// The id of `parent`'s path followed by `id`, whose hash is `hash`.
    fn child(parent: GlobalElementId, id: ElementId, hash: u64) -> GlobalElementId {
        let len = parent.len() + 1;
        GlobalElementId {
            path: Path(Some(Arc::new(PathNode {
                id,
                parent,
                len,
                #[cfg(any(test, feature = "test-support"))]
                flat: Default::default(),
            }))),
            hash,
        }
    }

    /// Whether `id` is the path `ids`.
    pub(crate) fn is(id: &GlobalElementId, ids: &[ElementId]) -> bool {
        id.matches_ids(ids)
    }

    /// Writes the path's element ids, dot separated.
    pub(crate) fn display(
        id: &GlobalElementId,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        for (i, element_id) in id.ids().into_iter().enumerate() {
            if i > 0 {
                write!(f, ".")?;
            }
            write!(f, "{}", element_id)?;
        }
        Ok(())
    }

    /// Whether two ids are the same path.
    pub(crate) fn same(a: &GlobalElementId, b: &GlobalElementId) -> bool {
        let (mut a, mut b) = (a, b);
        loop {
            if a.hash != b.hash {
                return false;
            }
            match (&a.path.0, &b.path.0) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if Arc::ptr_eq(x, y) {
                        return true;
                    }
                    if x.len != y.len || x.id != y.id {
                        return false;
                    }
                    a = &x.parent;
                    b = &y.parent;
                }
                _ => return false,
            }
        }
    }
}

impl GlobalElementId {
    /// How many element ids the path has.
    #[cfg(not(any(test, feature = "test-support")))]
    pub(crate) fn len(&self) -> usize {
        self.path.0.as_ref().map_or(0, |node| node.len)
    }

    /// How many element ids the path has. Public with test support, like the
    /// slice the id dereferences to there, whose method this one stands for.
    #[cfg(any(test, feature = "test-support"))]
    pub fn len(&self) -> usize {
        self.path.0.as_ref().map_or(0, |node| node.len)
    }

    /// The element id of the element itself.
    #[cfg(not(any(test, feature = "test-support")))]
    pub(crate) fn last(&self) -> Option<&ElementId> {
        self.path.0.as_ref().map(|node| &node.id)
    }

    /// The element id of the element itself. Public with test support, like
    /// the slice the id dereferences to there, whose method this one stands for.
    #[cfg(any(test, feature = "test-support"))]
    pub fn last(&self) -> Option<&ElementId> {
        self.path.0.as_ref().map(|node| &node.id)
    }

    /// The id of the path without its last element id.
    pub(crate) fn parent(&self) -> Option<&GlobalElementId> {
        self.path.0.as_ref().map(|node| &node.parent)
    }

    /// The id of the first `len` element ids of the path.
    pub(crate) fn ancestor(&self, len: usize) -> Option<&GlobalElementId> {
        let mut id = self;
        while id.len() > len {
            id = id.parent()?;
        }
        (id.len() == len).then_some(id)
    }

    /// Whether the path begins with every element id of `prefix`.
    pub(crate) fn has_prefix(&self, prefix: &GlobalElementId) -> bool {
        self.ancestor(prefix.len())
            .is_some_and(|ancestor| ancestor == prefix)
    }

    /// The element ids of the path, root first.
    pub(crate) fn ids(&self) -> SmallVec<[&ElementId; 16]> {
        let mut ids = SmallVec::new();
        let mut id = self;
        while let Some(node) = &id.path.0 {
            ids.push(&node.id);
            id = &node.parent;
        }
        ids.reverse();
        ids
    }

    /// Whether the path is `ids`.
    pub(crate) fn matches_ids(&self, ids: &[ElementId]) -> bool {
        if self.len() != ids.len() {
            return false;
        }
        let mut id = self;
        for expected in ids.iter().rev() {
            let Some(node) = &id.path.0 else {
                return false;
            };
            if node.id != *expected {
                return false;
            }
            id = &node.parent;
        }
        true
    }
}

/// The global ids handed out this frame and the last, by the hash of their
/// path.
///
/// An element whose path is the one it had on the frame before, which is
/// nearly every element, is given the id it had then. A new path is made of
/// the id of the path above it, found the same way, and one node.
#[derive(Default)]
pub(crate) struct GlobalIdCache {
    previous: FxHashMap<u64, GlobalElementId>,
    current: FxHashMap<u64, GlobalElementId>,
}

impl GlobalIdCache {
    /// The global id of `path`: one handed out already if there is one, a
    /// new one otherwise.
    pub(crate) fn get(&mut self, path: &crate::window::ElementIdStack) -> GlobalElementId {
        self.prefix(path, path.hashes(), path.len())
    }

    /// The global id of the first `len` ids of `ids`, `hashes` holding the
    /// hash of each prefix.
    fn prefix(&mut self, ids: &[ElementId], hashes: &[u64], len: usize) -> GlobalElementId {
        if len == 0 {
            return GlobalElementId::default();
        }
        let hash = hashes[len - 1];
        let path = &ids[..len];
        if let Some(id) = self.current.get(&hash)
            && id.matches_ids(path)
        {
            return id.clone();
        }
        let id = match self.previous.get(&hash) {
            Some(id) if id.matches_ids(path) => id.clone(),
            _ => {
                let parent = self.prefix(ids, hashes, len - 1);
                Path::child(parent, ids[len - 1].clone(), hash)
            }
        };
        self.current.insert(hash, id.clone());
        id
    }

    /// Keeps this frame's ids for the next one to find, and forgets the
    /// previous frame's.
    pub(crate) fn finish_frame(&mut self) {
        mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{GlobalElementId, GlobalIdCache};
    use crate::ElementId;
    use std::hash::{BuildHasher, BuildHasherDefault};
    use std::sync::Arc;

    fn same_node(a: &GlobalElementId, b: &GlobalElementId) -> bool {
        match (&a.path.0, &b.path.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    #[test]
    fn paths_share_their_ancestors() {
        let mut stack = crate::window::ElementIdStack::default();
        let mut cache = GlobalIdCache::default();
        stack.push(ElementId::from("root"));
        let root = cache.get(&stack);
        stack.push(ElementId::from("a"));
        let a = cache.get(&stack);
        stack.pop();
        stack.push(ElementId::from("b"));
        let b = cache.get(&stack);
        assert!(same_node(a.parent().unwrap(), &root));
        assert!(same_node(b.parent().unwrap(), &root));
        assert_eq!(b.len(), 2);
        assert!(b.has_prefix(&root));
        assert!(!b.has_prefix(&a));
        assert_eq!(b.last(), Some(&ElementId::from("b")));
        assert_eq!(
            b,
            GlobalElementId::new(&[ElementId::from("root"), ElementId::from("b")])
        );
        assert_eq!(b.to_string(), "root.b");
    }

    /// An id's hash is worked out from its path when it is made, so ids made
    /// apart from the same path have to agree, and ids of different paths
    /// must not be taken for one another even where their hashes would meet.
    #[test]
    fn global_ids_compare_and_hash_by_path() {
        let path = |ids: &[&'static str]| {
            GlobalElementId::new(
                &ids.iter()
                    .map(|id| ElementId::from(*id))
                    .collect::<Vec<_>>(),
            )
        };
        let hash = |id: &GlobalElementId| {
            BuildHasherDefault::<collections::FxHasher>::default().hash_one(id)
        };

        let a = path(&["root", "table", "row"]);
        let b = path(&["root", "table", "row"]);
        assert_eq!(a, b);
        assert_eq!(hash(&a), hash(&b));

        let c = path(&["root", "table", "cell"]);
        assert_ne!(a, c);

        // A path that happens to share another's hash is still a different id.
        let mut forged = c;
        forged.hash = a.hash;
        assert_ne!(a, forged);

        assert_eq!(GlobalElementId::default(), path(&[]));
    }

    /// A path asked for again this frame or the next gets the id already
    /// made for it, not a new copy; one unused for a whole frame is let go,
    /// and one whose hash is shared with another path is never mistaken
    /// for it.
    #[test]
    fn global_ids_are_reused_while_their_path_is_in_use() {
        let path = |ids: &[&'static str]| {
            let mut stack = crate::window::ElementIdStack::default();
            for id in ids {
                stack.push(ElementId::from(*id));
            }
            stack
        };
        let row = path(&["root", "table", "row"]);
        let cell = path(&["root", "table", "cell"]);
        let mut cache = GlobalIdCache::default();
        let hash = GlobalElementId::new(&row).hash;

        let first = cache.get(&row);
        assert!(same_node(&first, &cache.get(&row)));

        cache.finish_frame();
        let next = cache.get(&row);
        assert!(
            same_node(&first, &next),
            "a path in use last frame keeps its id"
        );

        cache.finish_frame();
        cache.finish_frame();
        let later = cache.get(&row);
        assert!(
            !same_node(&first, &later),
            "a path unused for a frame is let go"
        );
        assert_eq!(first, later);

        let mut forged = GlobalElementId::new(&cell);
        forged.hash = hash;
        cache.current.insert(hash, forged);
        assert!(cache.get(&row).matches_ids(&row));
    }
}
