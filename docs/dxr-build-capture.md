# Capturing the title's acceleration-structure builds

For the experimental host ray-tracing (DXR) backend: where the layer can
see what the title asks to be built, and the recorder that sits there.
Off by default (`VICTORY_RT_CAPTURE`). **Nothing here has run with a
title or on a GPU.** Status words: *host-tested* (tests pass on Linux),
*type-checked* (compiles for Windows, never executed), *assumed*.

## Where a build description is visible

The title builds its structures with middleware it ships
(`XboxBvhBuilder.dll`), which runs compute programs through our device.
"Lab" is the `xbox-hle-lab` reference (prose notes; another project's
static reading of that DLL, not ours).

| Place | What the code does today | A description there? |
|---|---|---|
| List slots 146 and 148 (newer lists' 0x490, 0x4a0) | written: enhanced barriers and list upload memory (`crates/victory-gpu/src/list.rs:1861-1865`, `1716`, `1219-1242`) | **no.** They are methods the builder *calls* (lab `CONSOLE-DLL-HARNESS.md:117-123`), not build entries, whatever `BARE-METAL-RUN-13-15.md:152` suggests. `list.rs:1221` says the builder puts its instance descriptions in that upload memory |
| The list's own ray-tracing methods (`ID3D12GraphicsCommandList4`: 11 console slots, 113-123, two later on newer lists; `list.rs:1769-1772`, `1851-1853`) | not written: a call logs `method not implemented` once with its slot and four arguments, and fails (`object.rs:313-330`) | only if the title calls them; which slot is the build is not known |
| Device: `ID3D12Device5`'s 9 slots (96-104) | not written, same log (`device.rs:388-445`) | no (size queries only) |
| Device feature queries | answered: 0, 1, 2, 3, 4, 7, 8, 23; any other warns `device feature query not implemented` (`device.rs:271-292`). The ray-tracing tier (27) is not answered | no |
| Title code calling the builder's exports | the DLL is mapped like the title and imports of it are bound to its exports by us (`crates/victory-loader/src/windows.rs:163-183`, `222-280`; `apps/victory/src/run.rs:118`, `154-177`, `313-315`) | **yes**: the exports take `D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC` (their C++ names carry the types; lab `CONSOLE-DLL-EXPORTS.md`, section 1) |
| The builder's dispatches (`DispatchX`, `list.rs:1111-1113`), root views and constants | run as translated compute | no: addresses derived from a description, not the description |
| A method looked up by name (`GetProcAddress` on a title image goes to the host, `crates/victory-hle/src/modules/kernel.rs:157-165`) | passes | not seen |

So the one reliable point is import binding: an import of a build method
is bound to a tap that calls the DLL's method with the same arguments
and, when that has returned, reads the description. It lives in `victory-hle`
(`crates/victory-hle/src/rtcapture.rs`) because imports are resolved
there and in the app; `victory-gpu` knows nothing of imports and holds
what the taps record into.

## What is implemented

| Part | Where | Status |
|---|---|---|
| Reading a description from guest memory through a callback: top level (instances as an array or through pointers, 64-byte layout), bottom level (triangles, boxes), flags, source, destination, scratch, post-build descriptions. PC's x64 layouts, offsets in the module text. Counts bounded (2^18 instances, 2^16 geometries); reads only what the description names, never vertex, index or transform data | `crates/victory-gpu/src/rtcapture/desc.rs` | host-tested (14 tests) |
| Every size, offset and enumeration value the reader uses, asserted at compile time against the `windows` crate's declarations of the same structures | same, `#[cfg(windows)]` | type-checked: the Windows check fails if one differs (tried with a wrong offset) |
| Refusals, each counted and logged once per distinct value: unknown level, layout, geometry type, index format, build flag bits outside PC's and the console's packed flag (0x1000000, lab `GHIDRA-BVH.md` 2.4), geometry flag bits, over the bound, null or unreadable memory | same | host-tested |
| Registry: destination to description, a generation per destination and a unique sequence number, the list it was recorded on, the bottom levels a top level names and which are known; 65,536 entries and 64 MiB at most, counted as `own_rt_capture_mb` | `rtcapture/registry.rs` | host-tested (5) |
| Copies: **not applied in a run.** Which argument is the source is not decided (`COPY_ORDER` is `None`), so a copy makes the registry forget both addresses and counts as unseen. With the order set, a copy in mode 1 gives the destination the source's description, and any other mode forgets the destination | `rtcapture/mod.rs` | host-tested, undecided and with either order |
| Counts and once-per-value log lines; totals at the first call, at every power of two of calls, every 10 s while calls arrive, 2 to 4 s after they stop (a timer thread, only with the switch on) and when the title exits, aborts or reports an unhandled exception through its own filter; `rtcapture::capture()` gives the registry to a later component | `rtcapture/mod.rs`, `crates/victory-hle/src/modules/kernel.rs` (`ending`) | host-tested (22, the copies' among them); the timer thread and the exit hooks are type-checked |
| Seven taps: build, batched build, copy, size query, three unpacking ("rehydrate") calls. Each counts the call, passes every argument on, shows the capture the call once the builder has returned, and hands the result back. An unpacking call that returns a failure records nothing and forgets the destinations it named. A call the builder throws out of records nothing and shows as `not returned` | `crates/victory-hle/src/rtcapture.rs` | host-tested with stand-in exports (7): argument order, results, a failing export, and a Rust unwind through a tap. Type-checked only: the binding into a real image, where win64 puts the arguments (the fifth is on the stack there, in a register on the test host), and an MSVC exception passing through a tap |
| Reading title memory (`ReadProcessMemory`: fails instead of faulting) and the wiring in `run.rs` | `rtcapture/mod.rs`, `apps/victory/src/run.rs:160`, `385` | type-checked |

An entry is removed when its destination is described again, when a
build there was refused, when an unpacking call naming it failed, when a
copy names it (see above), or when the runtime maps or unmaps its memory
(`crates/victory-gpu/src/watch.rs:829-831`, type-checked). **Nothing tells
us** that the title stopped using a structure inside one of its own
pools, and lengths are unknown, so an overlapping build at another
address removes nothing. A build that throws leaves the entry of its
destination as it was: right if the builder refused the build before
doing anything (what the lab describes), stale if it threw part-way,
which nothing here can tell; `not returned` counts them. The registry is
in recording order, not GPU order.

With the switch unset: no tap is installed, imports are bound to the same
addresses as before, no thread is started, the exit hooks do nothing, and
the only additions are debug-level log lines at binding and a
performance-log column that stays 0. With it set, nothing is built and
the title's calls reach the builder unchanged, before anything is read.

## Not verified until a title run

1. That the title imports these methods by these names (assumed from the
   lab's export table; in this tree only a test names the DLL among the
   title's imports, `run.rs:477`).
2. Geometry and instance layouts on the console (PC's assumed; the lab
   confirms only the description's 0x30 bytes and its flags at 0x0c). A
   run settles the geometry layout by the totals' `index format
   none/16/32`, `odd counts/strides/buffers`, the `a vertex format`
   lines and `a geometry that does not look like one` (which prints the
   geometry as read); the instance layout by `bottom levels named (not
   known)`.
3. `BuildRaytracingAccelerationStructure`'s 32-bit argument: taken as the
   post-build count, as in PC's method of that name, with one description
   (the lab calls it a description count, and also reads the function as
   passing exactly one description on).
4. The copy's argument order and modes. PC has (destination, source) and
   1 = compact; the lab writes (source, destination) and 1 = clone. No
   copy is applied until this is settled; the capture counts at which
   argument a structure was known to have been made, which over a run
   names the source (a source is a structure; a destination need not
   be).
5. Whether instance descriptions are final when the call is recorded.
6. The batched description (`...BATCHED_DESC2`): layout unknown, not read.
7. That an exception the builder throws passes through a tap to the
   title's handler (the taps are `system-unwind` and hold nothing across
   the call; never run on Windows).

## What a run logs, and what it decides

Run once with `VICTORY_RT_CAPTURE=dump` (as `=1`, plus the first 128
bytes of the first four batched descriptions). All lines start with
`ray-tracing capture:`.

| Line or figure | Means | For the design |
|---|---|---|
| no `build call tapped`, and no `a builder import ...` line | the title imports nothing from the builder by name | look for `method not implemented` on list slots 0x388-0x3e8; if neither, descriptions never reach us and structures must be decoded from guest nodes |
| no `build call tapped`, but `a builder import that is not one of the calls tapped` (whole name logged), `another signature`, `by ordinal` or `the title's own DLL does not serve` | the title imports the builder under names, signatures or a provenance not known here | a name or signature to add to the taps, not a design change |
| `build call tapped`, then no `totals` | the title never calls a tapped method | the fault is before the builder (feature queries, unwritten device slots); DXR cannot help yet |
| `totals`: `calls ... N not returned`, N above zero when the run has ended | the builder threw out of N calls (or they were running when the line was written) | nothing was recorded for them; see `crashes.log` for what was thrown. Not a basis for native builds until understood |
| `the builder failed an unpacking call` (one line per result), `calls failed` | the builder answered a failure | those structures do not exist; the fault is in what the builder was given or ran |
| `totals`: `batched` above zero | builds go through the console batch form | read `batch description, first bytes`: if it `reads as a public description`, the batch starts with one and reading it is a small change; if not, its layout has to be found first |
| `rehydrated` recorded with geometries | shipped structures arrive with their geometry named | bottom levels for the static world can be built natively |
| `description refused` with `rehydrate` and `counted but at null` | shipped structures name no geometry | those must be decoded from guest nodes or stay on software traversal |
| `description refused`, unknown flags or types | a console form PC does not have | add it only with evidence of its meaning; until then software |
| `bottom levels named N (M not known)`, M above zero | top levels use structures never described to us, or ones a copy made (not applied until item 4 is fixed) | hardware traversal of that top level is impossible until they are |
| `a vertex format` with `dxr=false`, `box sets` above zero | geometry DXR's triangles cannot take as it is | software intersection or fallback for those |
| `odd counts/strides/buffers` above zero in more than a few geometries, `a geometry that does not look like one` | the console's geometry description is probably not PC's | find its layout before anything is built from a record (item 2) |
| `a size query names a console stream description` | geometry may come packed | another console form to read first |
| copies `first/second known with every structure placed A/B` | which argument is the source: the one whose count is not zero, if the other is zero. Only this pair decides; the plain `first/second ... known` counts include copies made after a batched or unreadable build, where the known address can be the destination | fixes item 4: set `COPY_ORDER`, and copies are applied from then on. Both not zero, or both zero: not decided |
| `unseen` or `full` above zero (`by copies` says how many of the unseen are copies, which every copy is until item 4 is fixed) | the registry misses structures | not a basis for native builds until both are zero |

The last `totals` line is the run's: it is written again 2 to 4 s after
the last call and at exit. A process killed from outside, or a crash not
reported through the title's own filter, can lose the calls of its last
4 s from the totals (never more than half of all calls: there is a line
at each power of two).
