//! Platform-level data formats, shared by every Xbox Series title.
//!
//! Each module is pure (bytes in, bytes out, no I/O): [`bcpack`] (BCPack
//! streams to BC texture blocks), [`texture`] (the GPU's tiled texture
//! layouts and copies to and from linear rows) and [`xma`] (XMA2 audio).
//! Planned: Opus audio, and the zlib streams used by hardware decompression.

pub mod bcpack;
pub mod texture;
pub mod xma;
