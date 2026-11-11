//! Call stack depth for the formatter, the React renderers and `getAstValues`: where upstream
//! recurses until V8's stack runs out, the port stops where node does.
//!
//! These stages charge the crate's one model of node's stack, [`crate::depth`]: its budget
//! per thread ([`crate::depth::STACK`]; a site's cost is [`per_level`]`(n)`, where `n` is how
//! deep a fresh node 22 process nests that site's shape on its own before `RangeError:
//! Maximum call stack size exceeded`, solved by `work/api/depth/` in the findings), shared with
//! every other stage (a `format` or a React render called deep in a Rust caller's recursion
//! through the transform has what is left of it), and its stack growth ([`grow`] is
//! [`crate::depth::grow!`]).
//!
//! What this module adds is the fixed limit of these stages where the stack cannot grow
//! ([`crate::depth::can_grow`] false: the `stack-growth` feature off, wasm, or a thread whose
//! stack bounds are unknown): a second budget, the Rust stack a level takes in a debug build,
//! keeps each stage inside a quarter-spare 2 MiB stack (a fixed limit below node's, the
//! host's), as the other stages' fixed `MAX_*` limits do.

use std::cell::Cell;

/// The cost of one level of a shape that a fresh node process nests `n` levels deep and
/// throws `RangeError` for one more ([`crate::depth::per_level`]).
#[cfg_attr(not(test), allow(dead_code))]
pub const fn per_level(n: u64) -> u64 {
    crate::depth::per_level(n)
}

/// What a level may take of a 2 MiB thread's stack when the stack cannot grow: three
/// quarters of it, in bytes.
pub const FIXED_STACK: u64 = 3 * 512 * 1024;

thread_local! {
    /// How much of [`FIXED_STACK`] the open levels take on this thread (only where the stack
    /// cannot grow).
    static FIXED_USED: Cell<u64> = const { Cell::new(0) };
}

/// One open level of a recursion, charged on node's budget ([`crate::depth`]) and, where the
/// stack cannot grow, on [`FIXED_STACK`], until it is dropped.
#[must_use = "the level is given back when this is dropped"]
#[derive(Debug)]
pub struct Level {
    _node: crate::depth::Level,
    rust: u64,
}

impl Drop for Level {
    fn drop(&mut self) {
        FIXED_USED.with(|used| used.set(used.get().saturating_sub(self.rust)));
    }
}

/// Opens a level costing `node` of node's stack ([`crate::depth::STACK`]) and, where the
/// stack cannot grow, `rust` bytes of [`FIXED_STACK`] (the stack the level takes in a debug
/// build): `None` where node's stack, or the fixed one, would run out.
#[inline]
pub fn enter(node: u64, rust: u64) -> Option<Level> {
    let rust = if can_grow() { 0 } else { rust };
    let fits = FIXED_USED.with(|used| {
        let now = used.get().checked_add(rust).filter(|&r| r <= FIXED_STACK)?;
        used.set(now);
        Some(())
    });
    fits?;
    match crate::depth::enter(node) {
        Some(level) => Some(Level { _node: level, rust }),
        None => {
            FIXED_USED.with(|used| used.set(used.get().saturating_sub(rust)));
            None
        }
    }
}

/// Whether recursion grows the stack on this thread ([`crate::depth::can_grow`]).
#[inline]
pub fn can_grow() -> bool {
    crate::depth::can_grow()
}

/// Runs `f`, the next level of a recursion, on a stack with room for it
/// ([`crate::depth::grow!`]). Without stack growth `f` runs where it is (the fixed budget of
/// [`enter`] keeps the stack from overflowing).
#[inline]
pub fn grow<R>(f: impl FnOnce() -> R) -> R {
    crate::depth::grow!(f())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_of_n_levels_fits_and_one_more_does_not() {
        for n in [1, 2, 3, 818, 1_328, 4_096, 60_000] {
            let cost = per_level(n);
            let levels: Vec<Level> = (0..n).map(|_| enter(cost, 0).expect("fits")).collect();
            assert!(enter(cost, 0).is_none(), "{n}");
            drop(levels);
            assert_eq!(crate::depth::used(), 0);
            assert_eq!(FIXED_USED.with(Cell::get), 0);
        }
    }
}
