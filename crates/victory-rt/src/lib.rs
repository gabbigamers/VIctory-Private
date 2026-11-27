//! Ray-tracing validation that needs no GPU: the hardware-independent half
//! of the prototype for an experimental native DXR backend
//! (docs/dxr-validation-reference.md).
//!
//! * [`scene`]: a scene as DXR build descriptions state it, and the changes
//!   a frame brings.
//! * [`generate`]: seeded synthetic scenes and rays, one class per property
//!   under test. Nothing here comes from a title.
//! * [`mod@reference`]: the independent reference. Brute force over every
//!   triangle of every instance, exact where a classification must be, with
//!   the diagnostics a comparison needs to tell a defect from a difference
//!   DXR permits.
//! * [`node`], [`console`], [`traverse`]: a CPU model of the existing
//!   software path. The per-node test as `victory-shader` emulates it, a
//!   builder that writes a scene as console-format nodes, and a traversal
//!   over those bytes.
//! * [`compare`], [`results`]: the tolerance policy, the comparison and the
//!   result file a Windows run writes.
//!
//! `victory-shader`'s Windows tests hold the reference the node test here
//! was written beside; one of them compares the two word for word, so a
//! change to the lowering cannot leave [`node`] behind unnoticed.
//!
//! Everything is `f32` and `f64` arithmetic from the four basic operations,
//! and integers: the same inputs give the same bytes on every platform.

pub mod compare;
pub mod console;
pub mod exact;
pub mod generate;
pub mod node;
pub mod record;
pub mod reference;
pub mod results;
pub mod rng;
pub mod scene;
pub mod traverse;

pub use record::{Hit, Ray};

/// The alpha test that stands in for an alpha-tested material on geometry
/// not marked opaque: the barycentrics pick a cell of an 8 by 8 pattern,
/// and the cell and the primitive's index decide. In `f32`, as a shader
/// computes it. There is no clamp: a weight below zero, which an
/// implementation may report on an edge, is in cell -1, and the reference
/// counts 0 as a cell boundary for that reason
/// ([`reference::flag::ALPHA_NEAR`]). A negative zero is cell 0.
pub fn alpha_accepts(u: f32, v: f32, primitive_index: u32) -> bool {
    let cell = (u * 8.0).floor() as i32 + (v * 8.0).floor() as i32;
    (cell.wrapping_add(primitive_index as i32)) & 1 == 0
}

#[cfg(test)]
mod tests {
    use super::alpha_accepts;

    #[test]
    fn the_alpha_pattern_is_a_checkerboard_shifted_by_the_primitive() {
        assert!(alpha_accepts(0.0, 0.0, 0));
        assert!(!alpha_accepts(0.0, 0.0, 1));
        assert!(!alpha_accepts(0.125, 0.0, 0));
        assert!(alpha_accepts(0.125, 0.125, 0));
        assert!(alpha_accepts(0.124, 0.874, 0));
        assert!(!alpha_accepts(0.124, 0.874, 1));
        // A whole weight is cell 8.
        assert!(alpha_accepts(1.0, 0.0, 0));
        assert!(!alpha_accepts(0.99, 0.0, 0));
        // Just below zero is cell -1, and the parity turns; a negative zero
        // is zero.
        assert!(!alpha_accepts(-1e-9, 0.0, 0) && alpha_accepts(-1e-9, 0.0, 1));
        assert!(alpha_accepts(-0.0, -0.0, 0));
    }
}
