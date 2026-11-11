//! Call stack depth: where upstream's recursion is bounded only by V8's stack, the port's
//! stops where node's does.
//!
//! Upstream (markdown-it's block and inline rules, the tag grammar, the parser's
//! `handleToken`, `walk()`, the validator's `walkWithParents`, `Node.resolve` and `resolve`,
//! the transformer, the HTML renderer, `String()` of arrays, `JSON.stringify`) recurses until
//! V8's stack runs out, and then throws `RangeError: Maximum call stack size exceeded`. How
//! deep that is depends on what nests: each level of a shape puts the same frames on the stack,
//! and different shapes put different frames (a blockquote costs less than a list item, an
//! array in a tag's value less than a hash). This module models that stack as a budget per
//! thread, charged at each recursion site the port shares with upstream:
//!
//! - [`STACK`] is node's stack and every site's cost is a share of it, chosen so that each
//!   shape on its own, called from the top of a script, stops where a fresh node 22 process
//!   throws (the shape's "pure" limit; see `per_level`). Shapes that mix sites (a tag's value
//!   inside nested blockquotes, a list inside partials, a value resolved deep in a document)
//!   then stop near node's too, as V8's frames add up the same way. Nested calls share the
//!   budget as they share node's stack: a `debug()` deep in a document has less of it left
//!   than one at the top. The port's own loops (the parser's, the validator's, `resolve`'s,
//!   the HTML renderer's, `String()`'s and `JSON.stringify`'s walks) charge their path the
//!   same way without recursing.
//! - Where the recursion mirrors upstream's (the engine's rules, the grammar, the transform,
//!   the values' resolution, serde of the AST), `grow!` runs the next level on a new stack
//!   segment when the thread's stack runs low (the `stacker` crate), so that the budget, not
//!   the thread's stack, is the limit, whatever stack the caller runs on: a 2 MiB thread in a
//!   debug build reaches node's limits as a release build on the main thread does.
//! - Where the stack cannot grow ([`can_grow`]: the `stack-growth` feature off, a target
//!   without stack switching such as wasm, whose call stack is the host's, or a thread whose
//!   stack bounds are unknown), each of those recursions keeps the fixed limit it had before
//!   the budget, which fits a 2 MiB stack in a debug build with a quarter spare: there those
//!   limits, below node's, are the host's limit. The loops keep node's limits everywhere.
//!
//! Node's own limits move with how warm V8 is (optimized frames are smaller, and a call that
//! does much work before it nests warms V8 up within the call) and with the stack below the
//! call, so around each limit there is a band where a fresh process throws and a warmed-up
//! one does not; the port's limits are the fresh process's (`CONFORMANCE.md` gives the table).
//!
//! What it costs: a charge is a thread-local add and compare, and `grow!` reads the stack
//! pointer and the thread's stack limit; both run once per level of what nests (a blockquote,
//! a value in a tag, a node transformed), never per character, and segments are allocated only
//! past the thread's own stack.

use std::cell::Cell;

/// Node's stack in the budget's units: V8's default `--stack-size` (984 KB), in 2^-20ths of a
/// byte (fine enough that `n` levels of `per_level(n)` fit and `n + 1` do not for any `n` up to
/// a million). Only the ratios of the costs to it matter.
pub const STACK: u64 = 984 * 1024 * 1024 * 1024;

/// The cost of one level of a shape that node 22, in a fresh process at the top level of a
/// script, nests `n` levels deep and throws `RangeError` for one more: `n` levels fit
/// [`STACK`] and `n + 1` do not.
pub(crate) const fn per_level(n: u64) -> u64 {
    STACK / n
}

/// What a level of a shape that nests `b` deep costs beyond one of a shape that nests `a`
/// deep (`b < a`): the cost of a site that adds to a level of the first shape so that the two
/// together cost `per_level(b)` (a function parameter after a comma, which costs more than
/// the first parameter; a heading's transform, which costs more than a paragraph's).
pub(crate) const fn extra(a: u64, b: u64) -> u64 {
    per_level(b) - per_level(a)
}

thread_local! {
    /// How much of [`STACK`] the levels open on this thread take.
    static USED: Cell<u64> = const { Cell::new(0) };
}

/// One open level of a recursion, charged on the thread's budget: what it cost is given back
/// when it is dropped (also when an error or a panic unwinds through it).
#[must_use = "the level is given back when this is dropped"]
#[derive(Debug)]
pub(crate) struct Level {
    cost: u64,
}

impl Drop for Level {
    fn drop(&mut self) {
        USED.with(|used| used.set(used.get().saturating_sub(self.cost)));
    }
}

/// Opens a level costing `cost`: `None` where node's stack would run out (the caller then
/// throws `RangeError: Maximum call stack size exceeded`).
#[inline]
pub(crate) fn enter(cost: u64) -> Option<Level> {
    USED.with(|used| {
        let now = used.get().checked_add(cost).filter(|&now| now <= STACK)?;
        used.set(now);
        Some(Level { cost })
    })
}

/// How much of [`STACK`] the open levels take on this thread (0 outside any call).
pub(crate) fn used() -> u64 {
    USED.with(Cell::get)
}

/// What is left of [`STACK`] on this thread: what a loop that walks a path without recursing
/// may charge its path, as its levels would on node's stack.
pub(crate) fn left() -> u64 {
    STACK.saturating_sub(used())
}

/// What [`grow!`] keeps free below a level: at least this much stack is left when the next
/// level starts. A level of the port's deepest-framed recursions takes under 8 KiB in a debug
/// build; the margin covers what runs between two levels (a rule's frames, a schema's
/// transform written in Rust, a regular expression) with room to spare.
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
const RED_ZONE: usize = 256 * 1024;

/// The size of each stack segment [`grow!`] adds: a few hundred levels of the costliest
/// recursion in a debug build, so that a deep input switches stacks rarely.
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
const SEGMENT: usize = 4 * 1024 * 1024;

/// The address of a byte in the caller's frame: where its stack is (stacks grow down on every
/// target this crate supports).
#[inline(always)]
pub(crate) fn stack_address() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
thread_local! {
    /// The lowest address the stack in use on this thread may reach: 0 until asked, 1 where
    /// its bounds are unknown (no growth), else the thread's stack limit or, while a level
    /// runs on a segment [`grow!`] added, the segment's. (Kept here rather than asked of
    /// `stacker` at every level: reading it there costs a call and a lazy thread-local, many
    /// times a check of this.)
    static LIMIT: Cell<usize> = const { Cell::new(0) };
}

/// How far below the stack's limit a check of [`LIMIT`] is believed: farther than any stack
/// this crate's recursions run on, the limit is another stack's (a thread's first level run
/// on a segment of someone else's), and `stacker` is asked again.
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
const LIMIT_SPAN: usize = 1 << 30;

/// [`LIMIT`] for a stack at `here`, from `stacker` (which knows the thread's bounds, and the
/// segment's while it runs one).
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
#[cold]
fn learn_limit(here: usize) -> usize {
    let limit = stacker::remaining_stack().map_or(1, |left| here.saturating_sub(left).max(2));
    LIMIT.with(|l| l.set(limit));
    limit
}

/// Whether recursion grows the stack on this thread: the `stack-growth` feature is on, the
/// target can switch stacks (not wasm) and the thread's stack bounds are known (they are on
/// Linux, macOS, Windows and the BSDs). Where it cannot, the recursions that mirror upstream's
/// stop at their fixed limits (each module's `MAX_*` constants say which) instead of node's.
pub fn can_grow() -> bool {
    #[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
    {
        match LIMIT.with(Cell::get) {
            0 => learn_limit(stack_address()) != 1,
            limit => limit != 1,
        }
    }
    #[cfg(not(all(feature = "stack-growth", not(target_family = "wasm"))))]
    {
        false
    }
}

/// Runs `f`, the next level of a recursion, on a stack with at least `RED_ZONE` free: on a
/// new segment when the thread's stack (or the current segment) is short of it (see
/// [`grow!`]). The common case, room to spare, is a thread-local read and a compare.
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
#[inline]
pub(crate) fn grow_with<R>(f: impl FnOnce() -> R) -> R {
    let here = stack_address();
    let limit = LIMIT.with(Cell::get);
    if limit > 1 && here.wrapping_sub(limit).wrapping_sub(RED_ZONE) < LIMIT_SPAN {
        return f();
    }
    grow_slow(here, limit, f)
}

/// [`grow_with`] where the stack is short (or its limit not yet known).
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
#[cold]
#[inline(never)]
fn grow_slow<R>(here: usize, limit: usize, f: impl FnOnce() -> R) -> R {
    let limit = if limit > 1 && here.wrapping_sub(limit) < LIMIT_SPAN {
        limit
    } else {
        learn_limit(here)
    };
    if limit == 1 || here.saturating_sub(limit) >= RED_ZONE {
        return f();
    }
    /// Puts the stack's limit back when the level on the segment ends (or unwinds).
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            LIMIT.with(|l| l.set(self.0));
        }
    }
    stacker::grow(SEGMENT, move || {
        let _restore = Restore(limit);
        learn_limit(stack_address());
        f()
    })
}

/// `grow!(expr)`: evaluates `expr`, the next level of a recursion, on a stack with at least
/// `RED_ZONE` free (a new segment when the thread's stack, or the current segment, is short of
/// it). Without stack growth ([`can_grow`]), `expr` is evaluated where it is, with no frame
/// of its own (a closure would add one to every level of a debug build), and the caller's
/// fixed limit keeps the stack from overflowing.
#[cfg(all(feature = "stack-growth", not(target_family = "wasm")))]
macro_rules! grow {
    ($e:expr) => {
        $crate::depth::grow_with(|| $e)
    };
}
#[cfg(not(all(feature = "stack-growth", not(target_family = "wasm"))))]
macro_rules! grow {
    ($e:expr) => {
        $e
    };
}
pub(crate) use grow;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_of_n_levels_fits_and_one_more_does_not() {
        for n in [1, 2, 3, 300, 1_716, 2_914, 4_642, 9_820, 60_000] {
            let cost = per_level(n);
            assert_eq!(left() / cost, n);
            let levels: Vec<Level> = (0..n).map(|_| enter(cost).expect("fits")).collect();
            assert!(enter(cost).is_none(), "{n}");
            drop(levels);
            assert_eq!(used(), 0);
        }
    }

    #[test]
    fn levels_are_given_back_on_unwind() {
        let r = std::panic::catch_unwind(|| {
            let _a = enter(per_level(10)).unwrap();
            let _b = enter(per_level(10)).unwrap();
            panic!("unwinds");
        });
        assert!(r.is_err());
        assert_eq!(used(), 0);
    }

    #[test]
    fn growth_is_available_where_the_feature_and_target_allow_it() {
        let expected = cfg!(all(feature = "stack-growth", not(target_family = "wasm")));
        assert_eq!(can_grow(), expected);
        // a recursion far deeper than a 2 MiB thread holds, through grow
        fn down(n: usize) -> usize {
            let pad = std::hint::black_box([0u8; 512]);
            if n == 0 {
                return 1;
            }
            grow!(down(n - 1)) + usize::from(pad[n % 512])
        }
        if expected {
            let r = std::thread::Builder::new()
                .stack_size(2 << 20)
                .spawn(|| down(100_000))
                .unwrap()
                .join()
                .unwrap();
            assert!(r > 0);
        }
    }
}
