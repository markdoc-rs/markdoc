//! What is left of the old pipeline's source maps: the old path of [`LineIndex`], which
//! [`crate::next::parser`] still imports. The type lives in [`crate::positions`]; this module
//! goes when `next` moves to the root's files.

#[doc(no_inline)]
pub use crate::positions::LineIndex;
