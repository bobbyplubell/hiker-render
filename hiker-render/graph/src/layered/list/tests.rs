//! Port of `dagre/test/data/list-test.ts`. Test names mirror the TS so a
//! reviewer can diff against the original oracle.
//!
//! The TS tests rely on JS object *identity* (`expect(list.dequeue()).toBe(obj)`)
//! and on a single entry being movable between lists. In the arena port, an
//! entry's identity is its [`EntryId`]; we assert on those ids.

use super::*;

#[test]
fn dequeue_returns_undefined_with_an_empty_list() {
    let mut arena: ListArena<i32> = ListArena::new();
    let list = arena.new_list();
    assert_eq!(arena.dequeue(list), None);
}

#[test]
fn dequeue_unlinks_and_returns_the_first_entry() {
    let mut arena: ListArena<i32> = ListArena::new();
    let list = arena.new_list();
    let obj = arena.new_entry(0);
    arena.enqueue(list, obj);
    assert_eq!(arena.dequeue(list), Some(obj));
}

#[test]
fn dequeue_unlinks_and_returns_multiple_entries_in_fifo_order() {
    let mut arena: ListArena<i32> = ListArena::new();
    let list = arena.new_list();
    let obj1 = arena.new_entry(1);
    let obj2 = arena.new_entry(2);
    arena.enqueue(list, obj1);
    arena.enqueue(list, obj2);

    assert_eq!(arena.dequeue(list), Some(obj1));
    assert_eq!(arena.dequeue(list), Some(obj2));
}

#[test]
fn dequeue_unlinks_and_relinks_an_entry_if_it_is_re_enqueued() {
    let mut arena: ListArena<i32> = ListArena::new();
    let list = arena.new_list();
    let obj1 = arena.new_entry(1);
    let obj2 = arena.new_entry(2);
    arena.enqueue(list, obj1);
    arena.enqueue(list, obj2);
    arena.enqueue(list, obj1);

    assert_eq!(arena.dequeue(list), Some(obj2));
    assert_eq!(arena.dequeue(list), Some(obj1));
}

#[test]
fn dequeue_unlinks_and_relinks_an_entry_if_it_is_enqueued_on_another_list() {
    let mut arena: ListArena<i32> = ListArena::new();
    let list = arena.new_list();
    let list2 = arena.new_list();
    let obj = arena.new_entry(0);
    arena.enqueue(list, obj);
    arena.enqueue(list2, obj);

    assert_eq!(arena.dequeue(list), None);
    assert_eq!(arena.dequeue(list2), Some(obj));
}

#[test]
fn dequeue_can_return_a_string_representation() {
    // The TS renders `{entry: 1}` as JSON `{"entry":1}`. Our payload is the
    // observable content, so a payload that Displays as `{"entry":N}` mirrors
    // the original expectation exactly.
    struct Entry(i32);
    impl std::fmt::Display for Entry {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{{\"entry\":{}}}", self.0)
        }
    }

    let mut arena: ListArena<Entry> = ListArena::new();
    let list = arena.new_list();
    let e1 = arena.new_entry(Entry(1));
    let e2 = arena.new_entry(Entry(2));
    arena.enqueue(list, e1);
    arena.enqueue(list, e2);

    assert_eq!(arena.to_string(list), "[{\"entry\":1}, {\"entry\":2}]");
}
