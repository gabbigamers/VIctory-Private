//! Experimental: the title's ray tracing on the host's own ray-tracing
//! hardware, beside the software traversal that translated programs carry
//! (`victory_shader`'s emulation of the console's node instruction). Host
//! acceleration structures are built with D3D12 and traversed by inline
//! `RayQuery` (DXR 1.1). See docs/dxr-host-backend.md.
//!
//! **Nothing in a run uses this yet**, and none of it has run on hardware.
//! It is off unless `VICTORY_RT_BACKEND` asks for it, and the software
//! backend stays the reference and what every refused case uses.
//!
//! Everything that can be decided without a device is plain data and plain
//! logic, tested on any host:
//!
//! * [`select`]: which backend, from the device's capabilities and the
//!   switch;
//! * [`desc`]: what a structure is built from, and D3D12's rules for it;
//! * [`update`]: refit in place or build again;
//! * [`life`]: one structure's states, and when its memory may go;
//! * [`cache`]: the structures by their caller's identity, top levels
//!   holding the bottom levels they were built over;
//! * [`retire`]: memory waiting for the last submission that can read it;
//! * [`scratch`]: pooled, bounded memory for build scratch and instance
//!   descriptions;
//! * [`budget`]: how much the backend may hold;
//! * [`timing`]: GPU time of marked regions;
//! * [`stats`]: what was done and what was refused, counted.
//!
//! `device` (Windows only) is the thin layer of D3D12 calls over them.
//!
//! # The ordering contract
//!
//! Time is submissions: each list handed to the queue gets the next fence
//! value, complete when the list has run. The queue runs lists in the
//! order of their values, and a list's commands in recorded order.
//!
//! The contract is kept for **one list being recorded at a time**: its
//! builds and its reads are submitted, or dropped, together ([`cache`]).
//! Several lists open at once, as a title records, are not modelled; what
//! they would need is said in [`cache`] and is not written.
//!
//! 1. **Inputs ready.** The memory a build reads (positions, indices,
//!    transforms, boxes; instance descriptions) holds its contents before
//!    the build runs: written by the CPU before the list is submitted, or
//!    by GPU commands that are submitted, or recorded earlier in the same
//!    list. A build whose inputs something not yet submitted is still to
//!    write is refused ([`desc::Inputs::Pending`]). The inputs stay alive
//!    and unwritten until the build's submission is complete; after that
//!    the structure does not refer to them.
//! 2. **Build.** A build is recorded with a barrier on its result right
//!    after it. A build into memory the list may already have read (a
//!    refit in place, after a dispatch through a top level or a top-level
//!    build over it) has a barrier before it too: D3D12 orders reads and
//!    writes of a structure by such barriers in both directions. A top
//!    level is built only over bottom levels that may be read at that
//!    point, and its build reads them: they are held as by a reader. Builds
//!    in one list get scratch ranges of their own.
//! 3. **First trace.** A structure is read only after the barrier that
//!    follows its build: later in the same list, or in a later submission.
//!    A reader holds the structure, and through a top level its bottom
//!    levels, from when it is recorded until its submission is complete.
//! 4. **Change.** Building into a bottom level again (refit, rebuild,
//!    compacting copy) makes every top level built over it stale; a source
//!    at another content generation makes its structure stale. A stale
//!    structure is not read until it is built again, and neither is a top
//!    level over a stale bottom level, whether or not that bottom level
//!    has been given up since.
//! 5. **Release.** Memory a structure no longer uses, or the memory of a
//!    structure given up, is released only when the open list does not
//!    hold it and the highest fence value of a submission that may read or
//!    write it is complete. A bottom level given up stays while a top
//!    level built over it lives.

pub mod budget;
pub mod cache;
pub mod desc;
#[cfg(windows)]
pub mod device;
pub mod life;
pub mod retire;
pub mod scratch;
pub mod select;
pub mod stats;
pub mod timing;
pub mod update;
