//! Taps on the title's calls into its ray-tracing builder
//! (`VICTORY_RT_CAPTURE`, off by default; docs/dxr-build-capture.md).
//!
//! The title's acceleration structures are built by a middleware DLL it
//! ships, `XboxBvhBuilder.dll`: a C++ class whose methods take D3D12's
//! build descriptions and a command list, and then record compute
//! dispatches on that list. The DLL is mapped like the title and the
//! title's imports of it are bound to its exports, so the descriptions
//! pass from title code to title code and nothing of ours sees them. This
//! is the one place they can be seen without changing either side: when an
//! import of one of those methods is bound, [`WithRtCapture`] binds it to
//! a tap here instead, which calls the DLL's own method with the same
//! arguments and, once that has returned, lets `victory_gpu::rtcapture`
//! read the description (still the caller's memory) with what the method
//! returned. A call the builder throws out of or fails is therefore not
//! recorded as a structure built; it is counted.
//!
//! With the switch unset no tap is installed and every import is bound
//! exactly as before. A method found by name at run time (`GetProcAddress`)
//! is not tapped: nothing here sees it. With the switch on, every import
//! of the builder that is not tapped is logged with its whole name, so a
//! build method under a name not known here shows in the log.
//!
//! The methods and their argument types are the export names themselves:
//! C++ names carry their types. They are from another project's reading of
//! the DLL's export table (xbox-hle-lab, docs/CONSOLE-DLL-EXPORTS.md,
//! section 1), not from a DLL we have. An export of one of these methods
//! with any other signature is left alone and reported, since its
//! arguments would not be what a tap takes them for.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use victory_gpu::rtcapture::{self, Capture, desc::Read};
use victory_loader::{ImportResolver, Provenance, Resolution};
use victory_pe::ImportSymbol;

const DLL: &str = "xboxbvhbuilder.dll";
/// What follows a method's name in its export: the class, then the
/// signature.
const CLASS: &str = "@XboxBvhBuilder@@";

// The signatures, in the compiler's encoding. `QEAA`: a public member
// function, so the object comes first (in `rcx`); then the result (`X`
// none, `J` a 32-bit `long`); then the arguments: `PEAU..@@` a pointer to
// a structure, `PEBU..@@` to a constant one, `I` a 32-bit unsigned, `_K`
// a 64-bit unsigned, `1` the second argument type again, `W4..@@` an
// enumeration (32 bits), `PEB_K` a pointer to constant 64-bit unsigneds.
const BUILD: &str = "QEAAXPEAUID3D12GraphicsCommandList4@@\
PEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@I\
PEBUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC@@@Z";
const BATCHED: &str = "QEAAXPEAUID3D12GraphicsCommandList6@@I\
PEBUD3D12XBOX_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_BATCHED_DESC2@@@Z";
const COPY: &str = "QEAAXPEAUID3D12GraphicsCommandList4@@_K1\
W4D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE@@@Z";
const SIZES: &str = "QEAAXPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS@@\
PEBUD3D12XBOX_XDXR_STREAM_DESC@@\
PEAUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO@@@Z";
const REHYDRATE_LIST: &str = "QEAAJPEAUID3D12GraphicsCommandList8@@I\
PEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@@Z";
const REHYDRATE_LIST_2: &str = "QEAAJPEAUID3D12GraphicsCommandList8@@I\
PEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@PEB_K@Z";
const REHYDRATE_DEVICE: &str = "QEAAJPEAUID3D12Device11@@\
PEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@@Z";

type Ptr = *mut c_void;

/// The DLL's own methods, where the taps pass their calls on to: set when
/// a tap is installed, before any title code can call it.
static ORIGINALS: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];

struct Tap {
    method: &'static str,
    signature: &'static str,
    /// Its slot in [`ORIGINALS`].
    slot: usize,
    address: usize,
}

fn taps() -> [Tap; 7] {
    let tap = |slot, method, signature, address| Tap {
        method,
        signature,
        slot,
        address,
    };
    [
        tap(
            0,
            "BuildRaytracingAccelerationStructure",
            BUILD,
            build as *const () as usize,
        ),
        tap(
            1,
            "BuildRaytracingAccelerationStructureBatched2X",
            BATCHED,
            batched as *const () as usize,
        ),
        tap(
            2,
            "CopyRaytracingAccelerationStructure",
            COPY,
            copy as *const () as usize,
        ),
        tap(
            3,
            "GetRaytracingAccelerationStructurePrebuildInfo",
            SIZES,
            sizes as *const () as usize,
        ),
        tap(
            4,
            "RehydrateRaytracingAccelerationStructureBatchedX",
            REHYDRATE_LIST,
            rehydrate_list as *const () as usize,
        ),
        tap(
            5,
            "RehydrateRaytracingAccelerationStructureBatched2X",
            REHYDRATE_LIST_2,
            rehydrate_list_2 as *const () as usize,
        ),
        tap(
            6,
            "RehydrateRaytracingAccelerationStructureX",
            REHYDRATE_DEVICE,
            rehydrate_device as *const () as usize,
        ),
    ]
}

/// Lets the capture look at a call. A panic of ours stays here, reported
/// once: the title's call is passed on and answered whatever happens.
fn observe(f: impl FnOnce(&Capture, &mut Read<'_>)) {
    #[cfg(test)]
    let Some(f) = tests::observe_stand_in(f) else {
        return;
    };
    let Some(capture) = rtcapture::capture() else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| {
        f(capture, &mut rtcapture::title_memory())
    }))
    .is_err()
    {
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) {
            tracing::error!(
                "ray-tracing capture: reading a call failed; the call itself is unaffected"
            );
        }
    }
}

/// The method a tap passes its call on to, as a function of type `F` (a
/// function pointer), counted as entered; `None` if no tap was installed
/// for the slot. That cannot happen (a tap is bound only once its export
/// is stored); if it does, the title's call is not made, which is said
/// once.
///
/// # Safety
/// `F` is the function pointer type of the export installed in `slot`.
unsafe fn original<F: Copy>(slot: usize) -> Option<F> {
    const { assert!(size_of::<F>() == size_of::<usize>()) };
    let address = ORIGINALS[slot].load(Ordering::Acquire);
    // SAFETY: a function pointer is an address, and null is `None`; the
    // caller names the type.
    let found = unsafe { std::mem::transmute_copy::<usize, Option<F>>(&address) };
    if found.is_some() {
        observe(|capture, _| capture.entered());
    } else {
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) {
            tracing::error!(
                slot,
                "ray-tracing capture: a tap was called without the builder's method behind it; the call is not made"
            );
        }
    }
    found
}

// The taps. Each has its export's exact signature, passes the call on
// first and shows it to the capture when the builder has returned. An
// exception of the builder's passes through to the title, as it would
// without the tap (the builder throws for a destination it refuses, and
// the title catches that): nothing of ours is alive across the call, and
// nothing is recorded for a call that does not return. It was counted as
// entered, so it shows as one that did not return.

type BuildFn = unsafe extern "system-unwind" fn(Ptr, Ptr, Ptr, u32, Ptr);

/// `(list, description, post-build count, post-build descriptions)`, as
/// `ID3D12GraphicsCommandList4::BuildRaytracingAccelerationStructure` after
/// the list: one description.
unsafe extern "system-unwind" fn build(
    this: Ptr,
    list: Ptr,
    description: Ptr,
    postbuild_count: u32,
    postbuild: Ptr,
) {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<BuildFn>(0) }) else {
        return;
    };
    // SAFETY: as above.
    unsafe { next(this, list, description, postbuild_count, postbuild) };
    observe(|capture, read| {
        capture.build(
            read,
            list as u64,
            description as u64,
            postbuild_count,
            postbuild as u64,
        );
    });
}

type BatchedFn = unsafe extern "system-unwind" fn(Ptr, Ptr, u32, Ptr);

/// `(list, count, batch descriptions)`.
unsafe extern "system-unwind" fn batched(this: Ptr, list: Ptr, count: u32, descriptions: Ptr) {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<BatchedFn>(1) }) else {
        return;
    };
    // SAFETY: as above.
    unsafe { next(this, list, count, descriptions) };
    observe(|capture, read| capture.batched(read, list as u64, count, descriptions as u64));
}

type CopyFn = unsafe extern "system-unwind" fn(Ptr, Ptr, u64, u64, u32);

/// `(list, address, address, mode)`.
unsafe extern "system-unwind" fn copy(this: Ptr, list: Ptr, first: u64, second: u64, mode: u32) {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<CopyFn>(2) }) else {
        return;
    };
    // SAFETY: as above.
    unsafe { next(this, list, first, second, mode) };
    observe(|capture, _| capture.copy(list as u64, first, second, mode));
}

type SizesFn = unsafe extern "system-unwind" fn(Ptr, Ptr, Ptr, Ptr);

/// `(inputs, stream description, sizes out)`.
unsafe extern "system-unwind" fn sizes(this: Ptr, inputs: Ptr, stream: Ptr, out: Ptr) {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<SizesFn>(3) }) else {
        return;
    };
    // SAFETY: as above.
    unsafe { next(this, inputs, stream, out) };
    observe(|capture, read| capture.size_query(read, inputs as u64, stream as u64));
}

/// What a tap that has nowhere to pass its call on to answers: `E_FAIL`
/// ([`original`]).
const FAILED: i32 = 0x8000_4005_u32 as i32;

type RehydrateListFn = unsafe extern "system-unwind" fn(Ptr, Ptr, u32, Ptr) -> i32;

/// `(list, count, descriptions)`.
unsafe extern "system-unwind" fn rehydrate_list(
    this: Ptr,
    list: Ptr,
    count: u32,
    descriptions: Ptr,
) -> i32 {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<RehydrateListFn>(4) }) else {
        return FAILED;
    };
    // SAFETY: as above.
    let result = unsafe { next(this, list, count, descriptions) };
    observe(|capture, read| {
        capture.rehydrate(read, list as u64, count, descriptions as u64, result);
    });
    result
}

type RehydrateList2Fn = unsafe extern "system-unwind" fn(Ptr, Ptr, u32, Ptr, Ptr) -> i32;

/// `(list, count, descriptions, 64-bit values)`; what the values are is
/// not known, and they are passed on unread.
unsafe extern "system-unwind" fn rehydrate_list_2(
    this: Ptr,
    list: Ptr,
    count: u32,
    descriptions: Ptr,
    values: Ptr,
) -> i32 {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<RehydrateList2Fn>(5) }) else {
        return FAILED;
    };
    // SAFETY: as above.
    let result = unsafe { next(this, list, count, descriptions, values) };
    observe(|capture, read| {
        capture.rehydrate(read, list as u64, count, descriptions as u64, result);
    });
    result
}

type RehydrateDeviceFn = unsafe extern "system-unwind" fn(Ptr, Ptr, Ptr) -> i32;

/// `(device, description)`: one description, on no list.
unsafe extern "system-unwind" fn rehydrate_device(this: Ptr, device: Ptr, description: Ptr) -> i32 {
    // SAFETY: the export this tap stands in for, with its own arguments.
    let Some(next) = (unsafe { original::<RehydrateDeviceFn>(6) }) else {
        return FAILED;
    };
    // SAFETY: as above.
    let result = unsafe { next(this, device, description) };
    observe(|capture, read| capture.rehydrate(read, 0, 1, description as u64, result));
    result
}

/// What to bind an import to: `found`, or a tap in its place when the
/// capture is `on` and the import is one of the builder's methods read
/// here, served by the title's own DLL.
///
/// With the capture on, every import of the builder that is left as found
/// is logged where a release build shows it, with its whole name and why:
/// a run in which no build call is tapped then shows whether the title
/// imports a build method under a name not known here. That is a dozen
/// lines at most for each image that imports the builder.
fn bind(dll: &str, symbol: &ImportSymbol, found: Resolution, on: bool) -> Resolution {
    if !dll.eq_ignore_ascii_case(DLL) {
        return found;
    }
    let Resolution::Found {
        address,
        provenance: Provenance::Title,
    } = found
    else {
        if on {
            tracing::info!(
                %symbol,
                resolved = ?found,
                "ray-tracing capture: a builder import that the title's own DLL does not serve; not tapped"
            );
        }
        return found;
    };
    let ImportSymbol::Name { name, .. } = symbol else {
        if on {
            tracing::warn!(%symbol, "ray-tracing capture: a builder import by ordinal is not tapped");
        }
        return found;
    };
    // `?method@class@@signature`.
    let method = name
        .strip_prefix('?')
        .and_then(|rest| rest.split_once(CLASS));
    let taps = taps();
    let tap = method.and_then(|(method, _)| taps.iter().find(|t| t.method == method));
    let Some(tap) = tap else {
        if on {
            tracing::info!(
                %symbol,
                "ray-tracing capture: a builder import that is not one of the calls tapped; if it builds, its builds are not seen"
            );
        } else {
            tracing::debug!(%symbol, "ray-tracing capture: off; a builder import bound as it is");
        }
        return found;
    };
    if method.is_none_or(|(_, signature)| signature != tap.signature) {
        if on {
            tracing::warn!(
                %symbol,
                "ray-tracing capture: this build call has another signature than the one read; not tapped, its builds are not seen"
            );
        }
        return found;
    }
    if !on {
        tracing::debug!(
            method = tap.method,
            "ray-tracing capture: off; a build call bound as it is"
        );
        return found;
    }
    if cfg!(panic = "abort") {
        // An exception of the builder's could not pass through a tap to
        // the title's handler in such a build: it would end the process.
        tracing::warn!(
            method = tap.method,
            "ray-tracing capture: this build cannot pass exceptions through a tap; not tapped"
        );
        return found;
    }
    // Every importer binds to one export; another address would be a
    // second image of the DLL, which one tap cannot pass on to.
    let stored = &ORIGINALS[tap.slot];
    match stored.compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => tracing::info!(
            method = tap.method,
            "ray-tracing capture: build call tapped"
        ),
        Err(other) if other == address => {}
        Err(_) => {
            tracing::warn!(
                method = tap.method,
                "ray-tracing capture: a second image exports this build call; not tapped"
            );
            return found;
        }
    }
    Resolution::Found {
        address: tap.address,
        provenance: Provenance::Title,
    }
}

/// Resolves through `inner`, and with the capture on binds the title's
/// imports of its builder's build calls to taps that record and pass on.
pub struct WithRtCapture<'a> {
    pub inner: &'a dyn ImportResolver,
}

impl ImportResolver for WithRtCapture<'_> {
    fn resolve(&self, dll: &str, symbol: &ImportSymbol) -> Resolution {
        bind(
            dll,
            symbol,
            self.inner.resolve(dll, symbol),
            rtcapture::enabled(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn name(method: &str, signature: &str) -> ImportSymbol {
        ImportSymbol::Name {
            hint: 0,
            name: format!("?{method}{CLASS}{signature}"),
        }
    }

    fn title(address: usize) -> Resolution {
        Resolution::Found {
            address,
            provenance: Provenance::Title,
        }
    }

    /// The taps share their slots, so tests that install one take turns.
    static SLOTS: Mutex<()> = Mutex::new(());

    fn clear() {
        for slot in &ORIGINALS {
            slot.store(0, Ordering::Release);
        }
    }

    #[test]
    fn off_every_import_is_bound_as_it_was() {
        let _slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        for tap in taps() {
            let symbol = name(tap.method, tap.signature);
            assert_eq!(
                bind("XboxBvhBuilder.dll", &symbol, title(0x1234), false),
                title(0x1234)
            );
        }
        assert!(
            ORIGINALS
                .iter()
                .all(|slot| slot.load(Ordering::Acquire) == 0)
        );
    }

    #[test]
    fn on_the_build_calls_are_bound_to_their_taps_and_nothing_else_is() {
        let _slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let taps = taps();
        for (i, tap) in taps.iter().enumerate() {
            let symbol = name(tap.method, tap.signature);
            let own = 0x7000 + i;
            assert_eq!(
                bind("XBOXBVHBUILDER.DLL", &symbol, title(own), true),
                title(tap.address)
            );
            assert_eq!(ORIGINALS[tap.slot].load(Ordering::Acquire), own);
            // Another importer of the same export gets the same tap.
            assert_eq!(bind(DLL, &symbol, title(own), true), title(tap.address));
            // A second image's export is not ours to pass on to.
            assert_eq!(
                bind(DLL, &symbol, title(own + 0x100), true),
                title(own + 0x100)
            );
            assert_eq!(ORIGINALS[tap.slot].load(Ordering::Acquire), own);
        }
        // No two taps share a function or a slot.
        let mut addresses: Vec<usize> = taps.iter().map(|t| t.address).collect();
        addresses.sort_unstable();
        addresses.dedup();
        assert_eq!(addresses.len(), taps.len());
        assert!(taps.iter().enumerate().all(|(i, t)| t.slot == i));

        // The class's other methods, another DLL, an ordinal, and imports
        // that are not the title's own DLL's: as found.
        let other = [
            ImportSymbol::Name {
                hint: 0,
                name: "??0XboxBvhBuilder@@QEAA@PEAUID3D12Device11@@_N@Z".into(),
            },
            name("Set64BitMortonCodes", "QEAAX_N@Z"),
            name("CpuValidateResultX", BUILD),
            ImportSymbol::Ordinal(3),
        ];
        for symbol in &other {
            assert_eq!(
                bind(DLL, symbol, title(0x55), true),
                title(0x55),
                "{symbol}"
            );
        }
        // With every slot empty again, so that a tap would be installed
        // if these were taken for the title's own DLL's.
        clear();
        let build = name("BuildRaytracingAccelerationStructure", BUILD);
        assert_eq!(bind("d3d12_xs.dll", &build, title(0x55), true), title(0x55));
        assert_eq!(
            bind(DLL, &build, Resolution::Missing, true),
            Resolution::Missing
        );
        for provenance in [Provenance::Hle, Provenance::Host] {
            let other = Resolution::Found {
                address: 0x55,
                provenance,
            };
            assert_eq!(bind(DLL, &build, other, true), other, "{provenance:?}");
        }
        assert!(
            ORIGINALS
                .iter()
                .all(|slot| slot.load(Ordering::Acquire) == 0)
        );
        // The title's own is tapped from the same state: the cases above
        // were left alone for what they are.
        assert_eq!(bind(DLL, &build, title(0x55), true), title(taps[0].address));
        clear();
    }

    #[test]
    fn a_build_call_with_another_signature_is_left_alone() {
        let _slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        // The batched call's arguments under the single build's name, and
        // a build with one argument more.
        let extended = format!("{}_K@Z", BUILD.strip_suffix("@Z").unwrap());
        for signature in [BATCHED, extended.as_str(), ""] {
            let symbol = name("BuildRaytracingAccelerationStructure", signature);
            assert_eq!(
                bind(DLL, &symbol, title(0x99), true),
                title(0x99),
                "{signature}"
            );
        }
        assert_eq!(ORIGINALS[0].load(Ordering::Acquire), 0);
    }

    /// What the stand-in exports below were called with.
    static CALLS: Mutex<Vec<(&'static str, [u64; 5])>> = Mutex::new(Vec::new());

    fn called(what: &'static str, arguments: [u64; 5]) {
        CALLS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((what, arguments));
    }

    unsafe extern "system-unwind" fn own_build(this: Ptr, a: Ptr, b: Ptr, c: u32, d: Ptr) {
        called(
            "build",
            [this as u64, a as u64, b as u64, c.into(), d as u64],
        );
    }
    unsafe extern "system-unwind" fn own_batched(this: Ptr, a: Ptr, b: u32, c: Ptr) {
        called("batched", [this as u64, a as u64, b.into(), c as u64, 0]);
    }
    unsafe extern "system-unwind" fn own_copy(this: Ptr, a: Ptr, b: u64, c: u64, d: u32) {
        called("copy", [this as u64, a as u64, b, c, d.into()]);
    }
    unsafe extern "system-unwind" fn own_sizes(this: Ptr, a: Ptr, b: Ptr, c: Ptr) {
        called("sizes", [this as u64, a as u64, b as u64, c as u64, 0]);
    }
    /// `E_INVALIDARG`.
    const INVALID: i32 = 0x8007_0057_u32 as i32;
    /// What the three unpacking stand-ins answer unless a test says
    /// otherwise: two successes (one not zero) and a failure.
    const USUAL: [i32; 3] = [0x1234_5601, INVALID, 1];
    static RESULTS: [std::sync::atomic::AtomicI32; 3] =
        [const { std::sync::atomic::AtomicI32::new(0) }; 3];

    /// Sets what `own_list`, `own_list_2` and `own_device` answer.
    fn answer(results: [i32; 3]) {
        for (slot, result) in RESULTS.iter().zip(results) {
            slot.store(result, Ordering::Relaxed);
        }
    }

    unsafe extern "system-unwind" fn own_list(this: Ptr, a: Ptr, b: u32, c: Ptr) -> i32 {
        called("list", [this as u64, a as u64, b.into(), c as u64, 0]);
        RESULTS[0].load(Ordering::Relaxed)
    }
    unsafe extern "system-unwind" fn own_list_2(this: Ptr, a: Ptr, b: u32, c: Ptr, d: Ptr) -> i32 {
        called(
            "list 2",
            [this as u64, a as u64, b.into(), c as u64, d as u64],
        );
        RESULTS[1].load(Ordering::Relaxed)
    }
    unsafe extern "system-unwind" fn own_device(this: Ptr, a: Ptr, b: Ptr) -> i32 {
        called("device", [this as u64, a as u64, b as u64, 0, 0]);
        RESULTS[2].load(Ordering::Relaxed)
    }
    /// A builder that throws, as far as a host test can: a Rust unwind
    /// through the tap, not an MSVC exception.
    unsafe extern "system-unwind" fn own_build_throws(_: Ptr, _: Ptr, _: Ptr, _: u32, _: Ptr) {
        std::panic::resume_unwind(Box::new("refused"));
    }

    fn own() -> [usize; 7] {
        [
            own_build as *const () as usize,
            own_batched as *const () as usize,
            own_copy as *const () as usize,
            own_sizes as *const () as usize,
            own_list as *const () as usize,
            own_list_2 as *const () as usize,
            own_device as *const () as usize,
        ]
    }

    /// Puts the stand-in exports behind the taps.
    fn install(own: [usize; 7]) {
        for (slot, address) in ORIGINALS.iter().zip(own) {
            slot.store(address, Ordering::Release);
        }
    }

    /// Every tap passes each argument on in its place and hands back what
    /// the export returned. The capture is off here, as in any test run,
    /// so this is the passing on alone. It checks the order and the
    /// values, not where the title's convention puts them: on this host
    /// `system-unwind` is the System V convention, with all five arguments
    /// in registers, and on Windows the fifth is on the stack. That
    /// placement is the compiler's for the declared signatures, and is
    /// only type-checked.
    #[test]
    fn taps_pass_their_calls_on_unchanged() {
        let _slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let p = |n: usize| n as Ptr;
        // Not installed: nothing to call, and the calls say so.
        // SAFETY: no pointer is followed with the capture off.
        unsafe {
            build(p(1), p(2), p(3), 4, p(5));
            assert_eq!(rehydrate_list(p(1), p(2), 3, p(4)), FAILED);
        }
        assert!(CALLS.lock().unwrap().is_empty());

        install(own());
        answer(USUAL);
        let big = 0xfedc_ba98_7654_3210_u64;
        // SAFETY: the stand-ins follow no pointer either.
        let results = unsafe {
            build(p(0x11), p(0x12), p(0x13), 0xffff_fff4, p(0x15));
            batched(p(0x21), p(0x22), 0x23, p(0x24));
            copy(p(0x31), p(0x32), big, big - 1, 1);
            sizes(p(0x41), p(0x42), p(0x43), p(0x44));
            [
                rehydrate_list(p(0x51), p(0x52), 0x53, p(0x54)),
                rehydrate_list_2(p(0x61), p(0x62), 0x63, p(0x64), p(0x65)),
                rehydrate_device(p(0x71), p(0x72), p(0x73)),
            ]
        };
        assert_eq!(results, USUAL);
        assert_eq!(
            *CALLS.lock().unwrap(),
            [
                ("build", [0x11, 0x12, 0x13, 0xffff_fff4, 0x15]),
                ("batched", [0x21, 0x22, 0x23, 0x24, 0]),
                ("copy", [0x31, 0x32, big, big - 1, 1]),
                ("sizes", [0x41, 0x42, 0x43, 0x44, 0]),
                ("list", [0x51, 0x52, 0x53, 0x54, 0]),
                ("list 2", [0x61, 0x62, 0x63, 0x64, 0x65]),
                ("device", [0x71, 0x72, 0x73, 0, 0]),
            ]
        );
        CALLS.lock().unwrap().clear();
        clear();
    }

    /// A capture, and guest memory as ranges of bytes at addresses.
    type StandIn = (&'static Capture, Vec<(u64, Vec<u8>)>);

    thread_local! {
        /// A capture and guest memory for this thread's tap calls, in
        /// place of the run's (which is off in a test run, and whose
        /// reader has no title to read).
        static STAND_IN: std::cell::RefCell<Option<StandIn>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Runs `f` on this thread's stand-in, if it has one; else hands `f`
    /// back.
    pub(super) fn observe_stand_in<F: FnOnce(&Capture, &mut Read<'_>)>(f: F) -> Option<F> {
        STAND_IN.with_borrow(|stand_in| {
            let Some((capture, memory)) = stand_in else {
                return Some(f);
            };
            let mut read = |address: u64, out: &mut [u8]| {
                memory.iter().any(|(start, bytes)| {
                    let inside = address
                        .checked_sub(*start)
                        .and_then(|offset| bytes.get(offset as usize..)?.get(..out.len()));
                    inside.map(|found| out.copy_from_slice(found)).is_some()
                })
            };
            f(capture, &mut read);
            None
        })
    }

    fn words(parts: &[(usize, u64)]) -> Vec<u8> {
        parts
            .iter()
            .flat_map(|&(width, value)| value.to_le_bytes()[..width].to_vec())
            .collect()
    }

    /// A build description's bytes: destination, level, flags, count,
    /// array layout, array, source, scratch.
    fn description(destination: u64, level: u64, count: u64, array: u64) -> Vec<u8> {
        words(&[
            (8, destination),
            (4, level),
            (4, 0),
            (4, count),
            (4, 0),
            (8, array),
            (8, 0),
            (8, 0),
        ])
    }

    /// A capture of this test's own, with the order of a copy's addresses
    /// as given.
    fn own_capture(order: Option<victory_gpu::rtcapture::CopyOrder>) -> &'static Capture {
        use victory_gpu::account::Gauge;
        use victory_gpu::rtcapture::desc::Limits;
        use victory_gpu::rtcapture::registry::Bounds;
        let gauge: &'static Gauge = Box::leak(Box::new(Gauge::new()));
        Box::leak(Box::new(Capture::new(
            Limits::DEFAULT,
            Bounds::DEFAULT,
            gauge,
            false,
            order,
        )))
    }

    /// One triangle geometry (zeros but for its type and counts).
    fn one_geometry() -> Vec<u8> {
        words(&[
            (4, 0),
            (4, 1),
            (8, 0),
            (4, 57),
            (4, 6),
            (4, 36),
            (4, 24),
            (8, 0x100),
            (8, 0x200),
            (8, 12),
        ])
    }

    /// Each tap hands the capture the arguments its export has in those
    /// places, once the export has returned: checked through what the
    /// capture then knows.
    #[test]
    fn taps_show_the_capture_their_own_arguments() {
        use victory_gpu::rtcapture::CopyOrder;
        use victory_gpu::rtcapture::desc::Level;
        use victory_gpu::rtcapture::registry::Origin;

        let _slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        // The order decided, so that a copy leaves something to check its
        // arguments by.
        let capture = own_capture(Some(CopyOrder::DestinationFirst));
        // One triangle geometry, used by every bottom level; one instance
        // naming bottom level 0xb000.
        let geometry = one_geometry();
        let mut instance = vec![0; 0x38];
        instance.extend(words(&[(8, 0xb000)]));
        let mut unpacked = description(0xc000, 1, 1, 0x2000);
        unpacked.extend(description(0xd000, 1, 1, 0x2000));
        let memory = vec![
            (0x1000, description(0xb000, 1, 1, 0x2000)),
            (0x2000, geometry),
            (0x3000, description(0xa000, 0, 1, 0x4000)),
            (0x4000, instance),
            (
                0x5000,
                words(&[(8, 0x9990), (4, 0), (4, 0), (8, 0x9998), (4, 3), (4, 0)]),
            ),
            (0x6000, unpacked),
            (0x7000, description(0xe000, 1, 1, 0x2000)),
            (0x8000, description(0xf000, 1, 1, 0x2000)),
            // The inputs of a size query: a top level.
            (0x9000, description(0, 0, 5, 0x4000)[8..].to_vec()),
        ];
        STAND_IN.set(Some((capture, memory)));
        // Not installed: no builder was called, so nothing was built and
        // nothing is recorded or counted.
        let p = |n: usize| n as Ptr;
        // SAFETY: no export is installed, so nothing is called; the
        // stand-in reads only the bytes above.
        unsafe {
            build(p(1), p(0x111), p(0x1000), 2, p(0x5000));
            assert_eq!(rehydrate_list(p(1), p(0x333), 2, p(0x6000)), FAILED);
        }
        assert_eq!(capture.counts(), Default::default());
        assert!(capture.with_registry(|r| r.is_empty()));

        install(own());
        // Every unpacking call succeeds, one with a success that is not 0.
        answer([1, 0, 0]);
        // SAFETY: the stand-in exports follow no pointer, and the stand-in
        // memory is read only inside the bytes above.
        unsafe {
            build(p(1), p(0x111), p(0x1000), 2, p(0x5000));
            build(p(1), p(0x222), p(0x3000), 0, p(0));
            rehydrate_list(p(1), p(0x333), 2, p(0x6000));
            rehydrate_list_2(p(1), p(0x444), 1, p(0x7000), p(0x7777));
            rehydrate_device(p(1), p(0x555), p(0x8000));
            batched(p(1), p(0x666), 3, p(0x1000));
            // PC's order: to 0x1_b000 from 0xb000, compacting.
            copy(p(1), p(0x777), 0x1_b000, 0xb000, 1);
            sizes(p(1), p(0x9000), p(0x9999), p(0));
        }
        STAND_IN.set(None);
        CALLS.lock().unwrap().clear();
        clear();

        let counts = capture.counts();
        assert_eq!((counts.entered, counts.returned), (8, 8));
        assert_eq!((counts.failed_calls, counts.unseen), (0, 3));
        assert_eq!(
            (
                counts.build_calls,
                counts.recorded_top,
                counts.recorded_bottom
            ),
            (2, 1, 5)
        );
        assert_eq!(
            (counts.rehydrate_calls, counts.rehydrate_descriptions),
            (3, 4)
        );
        assert_eq!((counts.batched_calls, counts.batched_descriptions), (1, 3));
        assert_eq!(
            (
                counts.copy_calls,
                counts.copies_applied,
                counts.copies_second_known
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (
                counts.size_queries,
                counts.size_queries_top,
                counts.size_queries_with_stream
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (counts.refused, counts.postbuild, counts.instances),
            (0, 2, 1)
        );
        assert_eq!((counts.references, counts.references_unknown), (1, 0));
        capture.with_registry(|r| {
            let lists: Vec<(u64, u64)> = r.iter().map(|(at, entry)| (at, entry.list)).collect();
            assert_eq!(
                lists,
                [
                    (0xa000, 0x222),
                    (0xb000, 0x111),
                    (0xc000, 0x333),
                    (0xd000, 0x333),
                    (0xe000, 0x444),
                    // A device call is on no list.
                    (0xf000, 0),
                    (0x1_b000, 0x777),
                ]
            );
            let built = r.get(0xb000).unwrap();
            assert_eq!(built.origin, Origin::Build);
            assert_eq!(built.postbuild.len(), 2);
            assert_eq!(
                (built.postbuild[1].destination, built.postbuild[1].kind),
                (0x9998, 3)
            );
            assert!(
                matches!(&built.description.level, Level::Bottom(b) if b.geometries.len() == 1)
            );
            assert_eq!(r.get(0xd000).unwrap().origin, Origin::Rehydrate);
            assert!(matches!(
                r.get(0x1_b000).unwrap().origin,
                Origin::Copy { from: 0xb000, .. }
            ));
            assert_eq!(r.get(0xa000).unwrap().bottom_levels(), [0xb000]);
        });
    }

    /// The review's cases: a builder that answers with a failure, and one
    /// that throws. Neither built what it was asked to, and neither is
    /// recorded as if it had.
    #[test]
    fn a_call_the_builder_fails_or_throws_out_of_is_not_recorded_as_built() {
        let _slots = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let capture = own_capture(None);
        let memory = vec![
            (0x1000, description(0xb000, 1, 1, 0x2000)),
            (0x2000, one_geometry()),
            // The same destination again, and two more, to unpack.
            (0x6000, description(0xb000, 1, 0, 0)),
            (0x7000, description(0xc000, 1, 1, 0x2000)),
            (0x8000, description(0xd000, 1, 1, 0x2000)),
        ];
        STAND_IN.set(Some((capture, memory)));
        install(own());
        let p = |n: usize| n as Ptr;
        // SAFETY: as in the test above.
        unsafe { build(p(1), p(0x111), p(0x1000), 0, p(0)) };
        assert!(capture.with_registry(|r| r.get(0xb000).is_some()));

        // Each unpacking export answers a failure of its own: the tap
        // hands that back, the description is not recorded, and what was
        // known of the destination is forgotten, since how far the builder
        // got is not.
        let failures = [INVALID, FAILED, i32::MIN];
        answer(failures);
        // SAFETY: as above.
        let results = unsafe {
            [
                rehydrate_list(p(1), p(0x333), 1, p(0x6000)),
                rehydrate_list_2(p(1), p(0x444), 1, p(0x7000), p(0)),
                rehydrate_device(p(1), p(0x555), p(0x8000)),
            ]
        };
        assert_eq!(results, failures);
        let counts = capture.counts();
        assert_eq!((counts.rehydrate_calls, counts.failed_calls), (3, 3));
        assert_eq!((counts.recorded_bottom, counts.refused), (1, 0));
        assert_eq!((counts.removed_unread, counts.unseen), (1, 0));
        assert!(capture.with_registry(|r| r.is_empty()));

        // The export throws: the unwind passes through the tap, nothing is
        // recorded, and the entry of an earlier build stays as it was.
        // SAFETY: as above.
        unsafe { build(p(1), p(0x111), p(0x1000), 0, p(0)) };
        ORIGINALS[0].store(own_build_throws as *const () as usize, Ordering::Release);
        let thrown = std::panic::catch_unwind(|| {
            // SAFETY: as above.
            unsafe { build(p(1), p(0x999), p(0x1000), 0, p(0)) }
        });
        assert_eq!(thrown.unwrap_err().downcast_ref::<&str>(), Some(&"refused"));
        let counts = capture.counts();
        assert_eq!((counts.entered, counts.returned), (6, 5));
        assert_eq!(counts.unreturned(), 1);
        assert_eq!((counts.build_calls, counts.recorded_bottom), (2, 2));
        capture.with_registry(|r| {
            let before = r.get(0xb000).unwrap();
            assert_eq!((before.list, before.generation), (0x111, 0));
        });
        STAND_IN.set(None);
        CALLS.lock().unwrap().clear();
        clear();
    }

    /// The names the taps answer to are the ones the DLL exports: spelled
    /// out once here, whole, as the export table has them.
    #[test]
    fn the_tapped_names_are_the_exports() {
        let whole = |method: &str, signature: &str| format!("?{method}{CLASS}{signature}");
        let taps = taps();
        assert_eq!(
            whole(taps[0].method, taps[0].signature),
            "?BuildRaytracingAccelerationStructure@XboxBvhBuilder@@QEAAXPEAUID3D12GraphicsCommandList4@@PEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@IPEBUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC@@@Z"
        );
        assert_eq!(
            whole(taps[2].method, taps[2].signature),
            "?CopyRaytracingAccelerationStructure@XboxBvhBuilder@@QEAAXPEAUID3D12GraphicsCommandList4@@_K1W4D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE@@@Z"
        );
        assert_eq!(
            whole(taps[5].method, taps[5].signature),
            "?RehydrateRaytracingAccelerationStructureBatched2X@XboxBvhBuilder@@QEAAJPEAUID3D12GraphicsCommandList8@@IPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@PEB_K@Z"
        );
    }
}
