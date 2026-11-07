//! The tab order of a frame, sorted only when it is read.
//!
//! Every focusable element inserts itself into its frame's tab stops as it
//! is painted, and each insert into a sum tree copies the nodes on its path.
//! The order is read only to move focus with the keyboard, so the inserts are
//! collected as they come and the tree is built from them on the first read.

use std::cell::{OnceCell, RefCell};
use std::ops::Deref;
use sum_tree::{KeyedItem, SumTree};

pub(crate) struct LazyOrder<T: KeyedItem> {
    pending: RefCell<Vec<T>>,
    tree: OnceCell<SumTree<T>>,
}

impl<T: KeyedItem> Default for LazyOrder<T> {
    fn default() -> Self {
        Self {
            pending: RefCell::default(),
            tree: OnceCell::new(),
        }
    }
}

impl<T: KeyedItem> std::fmt::Debug for LazyOrder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyOrder")
            .field("pending", &self.pending.borrow().len())
            .field("built", &self.tree.get().is_some())
            .finish()
    }
}

impl<T> LazyOrder<T>
where
    T: KeyedItem,
    for<'a> T::Summary: sum_tree::Summary<Context<'a> = ()>,
{
    /// Inserts `item`, replacing an item with the same key, as
    /// [`SumTree::insert_or_replace`] does.
    pub(crate) fn insert_or_replace(&mut self, item: T, cx: ()) {
        match self.tree.get_mut() {
            Some(tree) => {
                tree.insert_or_replace(item, cx);
            }
            None => self.pending.get_mut().push(item),
        }
    }

    /// Removes every item, keeping the room collected for them.
    pub(crate) fn clear(&mut self) {
        self.pending.get_mut().clear();
        self.tree.take();
    }

    fn tree(&self) -> &SumTree<T> {
        self.tree.get_or_init(|| {
            let mut pending = self.pending.borrow_mut();
            // Stable, so of two items with one key the later one is last and
            // is the one kept, as it would have replaced the earlier one.
            pending.sort_by_cached_key(|item| item.key());
            let mut items: Vec<T> = Vec::with_capacity(pending.len());
            for item in pending.drain(..) {
                match items.last_mut() {
                    Some(last) if last.key() == item.key() => *last = item,
                    _ => items.push(item),
                }
            }
            SumTree::from_iter(items, ())
        })
    }
}

impl<T> Deref for LazyOrder<T>
where
    T: KeyedItem,
    for<'a> T::Summary: sum_tree::Summary<Context<'a> = ()>,
{
    type Target = SumTree<T>;

    fn deref(&self) -> &SumTree<T> {
        self.tree()
    }
}
