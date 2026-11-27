//! Traversal workloads: the translation-time half of the ray-tracing
//! compatibility layer (docs/dxr-region-recognition.md). A loop of a
//! program that walks a console-format structure is recognised as one
//! [`Description`], which names no path that could run it, or is given a
//! [`Refusal`]. Nothing consumes a description yet: no translation
//! depends on this module.

pub(crate) mod asm;
pub mod census;
pub mod describe;
pub mod recognise;
pub mod region;
pub(crate) mod terms;
pub mod walk;

pub use census::Census;
pub use describe::{Class, Description, Refusal};
pub use recognise::{Finding, Parts, Report, recognise};

#[cfg(test)]
mod tests;
