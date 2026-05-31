//! Simple intrusive doubly-linked list — a port of `dagre/lib/data/list.ts`
//! (itself derived from Cormen et al., "Introduction to Algorithms").
//!
//! # Why an arena
//!
//! The TS version is a sentinel-based doubly-linked list whose entries are
//! plain objects tagged with `_prev`/`_next` links. The defining behaviour that
//! makes it *intrusive* is that **the same entry object can live in (at most)
//! one list at a time and can be moved between lists**: `enqueue` first unlinks
//! the entry from wherever it currently is (`if (entry._prev && entry._next)`),
//! and greedy-fas relies on this — a single FAS entry hops between bucket lists
//! as its in/out degree changes.
//!
//! In Rust we model this with an **arena**: a single [`ListArena`] owns every
//! entry ([`Slot`]) in a `Vec`. Each [`List`] is just a sentinel index into the
//! arena; entries are referred to by their arena index ([`EntryId`]). This gives
//! us the JS object-identity semantics (an entry is one slot, no matter which
//! list it is threaded into) without `Rc<RefCell<…>>`, keeps greedy-fas's usage
//! clean (it can mutate an entry's payload in place via the arena while the
//! entry sits in a bucket), and stays deterministic. A sentinel uses
//! [`SENTINEL_NONE`] as its "no payload" marker; entries carry a `P` payload.

/// Index of an entry (or sentinel) within a [`ListArena`].
pub type EntryId = usize;

/// A slot in the arena: either a list sentinel or a payload-carrying entry.
///
/// `prev`/`next` are arena indices. When an entry is **not** linked into any
/// list, both are [`UNLINKED`]; the JS port models this with `delete
/// entry._next/_prev` (so `entry._prev && entry._next` is falsy).
#[derive(Debug)]
struct Slot<P> {
    prev: EntryId,
    next: EntryId,
    /// `None` for sentinels; `Some(payload)` for entries.
    payload: Option<P>,
}

/// Marker for an entry that is currently unlinked from every list. Mirrors the
/// JS `delete entry._next`/`delete entry._prev`.
const UNLINKED: EntryId = usize::MAX;

/// An arena owning the storage for one or more intrusive doubly-linked lists.
///
/// Create [`List`]s with [`new_list`](ListArena::new_list) and entries with
/// [`new_entry`](ListArena::new_entry). All link operations take the arena
/// explicitly, so entries can be shared between lists (matching the TS, where an
/// entry object can be moved from one `List` to another).
#[derive(Debug, Default)]
pub struct ListArena<P> {
    slots: Vec<Slot<P>>,
}

impl<P> ListArena<P> {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Create a new, empty list and return its handle. The list's sentinel
    /// initially points to itself (`sentinel._next = sentinel._prev =
    /// sentinel`).
    pub fn new_list(&mut self) -> List {
        let id = self.slots.len();
        self.slots.push(Slot {
            prev: id,
            next: id,
            payload: None,
        });
        List { sentinel: id }
    }

    /// Allocate a new (unlinked) entry holding `payload` and return its id.
    pub fn new_entry(&mut self, payload: P) -> EntryId {
        let id = self.slots.len();
        self.slots.push(Slot {
            prev: UNLINKED,
            next: UNLINKED,
            payload: Some(payload),
        });
        id
    }

    /// Immutable access to an entry's payload.
    ///
    /// # Panics
    /// Panics if `id` is a sentinel (has no payload).
    pub fn payload(&self, id: EntryId) -> &P {
        self.slots[id]
            .payload
            .as_ref()
            .expect("payload() called on a sentinel")
    }

    /// Mutable access to an entry's payload.
    ///
    /// # Panics
    /// Panics if `id` is a sentinel (has no payload).
    pub fn payload_mut(&mut self, id: EntryId) -> &mut P {
        self.slots[id]
            .payload
            .as_mut()
            .expect("payload_mut() called on a sentinel")
    }

    /// Whether `entry` is currently linked into some list. Mirrors the JS guard
    /// `entry._prev && entry._next`.
    fn is_linked(&self, entry: EntryId) -> bool {
        self.slots[entry].prev != UNLINKED && self.slots[entry].next != UNLINKED
    }

    /// `unlink(entry)` — splice `entry` out of its list and mark it unlinked.
    fn unlink(&mut self, entry: EntryId) {
        let prev = self.slots[entry].prev;
        let next = self.slots[entry].next;
        self.slots[prev].next = next;
        self.slots[next].prev = prev;
        self.slots[entry].prev = UNLINKED;
        self.slots[entry].next = UNLINKED;
    }

    /// `list.dequeue()` — unlink and return the entry at `sentinel._prev` (the
    /// tail), or `None` if the list is empty.
    pub fn dequeue(&mut self, list: List) -> Option<EntryId> {
        let sentinel = list.sentinel;
        let entry = self.slots[sentinel].prev;
        if entry != sentinel {
            self.unlink(entry);
            Some(entry)
        } else {
            None
        }
    }

    /// `list.enqueue(entry)` — insert `entry` at the head (`sentinel._next`),
    /// unlinking it first if it is currently in a (possibly different) list.
    pub fn enqueue(&mut self, list: List, entry: EntryId) {
        let sentinel = list.sentinel;
        if self.is_linked(entry) {
            self.unlink(entry);
        }
        let old_next = self.slots[sentinel].next;
        self.slots[entry].next = old_next;
        self.slots[old_next].prev = entry;
        self.slots[sentinel].next = entry;
        self.slots[entry].prev = sentinel;
    }
}

/// A handle to one intrusive doubly-linked list within a [`ListArena`].
///
/// Cheap to copy — it is just the arena index of the list's sentinel. All
/// operations are methods on the owning [`ListArena`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct List {
    sentinel: EntryId,
}

impl<P: std::fmt::Display> ListArena<P> {
    /// `list.toString()` — entries from `sentinel._prev` walking via `_prev`,
    /// rendered `[a, b, …]`. Each entry is rendered via its payload's
    /// [`Display`](std::fmt::Display) (the TS renders entries as JSON with the
    /// `_prev`/`_next` links filtered out; here the payload *is* the entry's
    /// observable content).
    pub fn to_string(&self, list: List) -> String {
        let sentinel = list.sentinel;
        let mut strs: Vec<String> = Vec::new();
        let mut curr = self.slots[sentinel].prev;
        while curr != sentinel {
            strs.push(format!("{}", self.payload(curr)));
            curr = self.slots[curr].prev;
        }
        format!("[{}]", strs.join(", "))
    }
}

#[cfg(test)]
mod tests;
