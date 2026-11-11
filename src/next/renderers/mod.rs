//! Port of upstream Markdoc v0.5.11's `src/renderers/index.ts`.
//!
//! Owner: the semantics agent (`html`); the api agent (`react`, `react_static`).
//!
//! `renderers.html` ([`html`]), `renderers.react` ([`react`], React behind a trait) and
//! `renderers.reactStatic` ([`react_static`]).

pub mod html;
pub mod react;
pub mod react_static;
