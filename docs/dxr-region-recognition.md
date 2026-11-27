# Recognising traversal regions

The translation-time half of the ray-tracing compatibility layer
([dxr-architecture.md](dxr-architecture.md), "The layer" and the first
two rows of "Where each piece lives"): from a loop of a translated
compute program to a workload description that names no path, or to a
named refusal. `ir:` is `crates/victory-shader/src/ir.rs`, `wl/` is
`crates/victory-shader/src/workload/`.

**State: implemented; tested (host tests); the synthetic programs'
containers accepted by Microsoft's validator on Linux; the census command
type-checked for Windows only; nothing hardware-validated.** No title
program was read: **every statement below is about the synthetic programs
of `wl/walk.rs`**, one way a compiler could lay such a loop out. Whether
any title loop has the accepted shape is unknown until the census runs.
Nothing consumes a description: no translation depends on this code, and
with it present every container the existing tests emit is unchanged.

## The analyses

| Analysis | Where | What it gives |
|---|---|---|
| Natural loops | `ir::natural_loops`, on `ir::latches` and `ir::back_edge_places` | every loop as a value: header, place among the loops of its header, blocks, its own back edges. **One rule**: the loop limits (`emit/lower/limit.rs`) now call the same two functions, moved out of the lowering unchanged |
| Region of a loop | `ir::region` | the blocks that enter it, its exit edges, the values it carries (the header's phis), values live in and live out, as the machine reads registers |
| Data reads | `wl/region.rs`, `data_reads` | an instruction's reads without the two that are not data in one lane's run: EXEC, and the old value of a register an unguarded write overwrites |
| Memory touched | `wl/region.rs`, `touches` | each access of a set of blocks as group-shared (`ds_*`: where a walk keeps its stack), tracked (through a descriptor or an address traced to a root parameter), raw (an address computed from data), or the node test (through a tracked descriptor, or raw); and whether it writes |
| Terms | `wl/terms.rs` | each SSA value as one lane computes it: instruction names over operands, loads by site, and a *pick* by a per-lane condition (`structure::Bool`) at a guarded write, a `v_cndmask_b32` and the join of an `if`. Equal computations are equal terms. One pass, no recursion |

## The description (`wl/describe.rs`)

| Field | Content today |
|---|---|
| `region`, `node` | the loop as `ir::Region`; the node instruction's site |
| `ray` | origin and direction (three values each, set before the loop); the inverse, proven `v_rcp_f32` of each direction component; the minimum and its compare form; the extent: its compare form, what it starts as, whether it shrinks to each accepted distance, and at which extents the accept passes the node test's answer for a *missed* triangle (`miss_word`; "A candidate is not a hit" below). A form is strict or inclusive, ordered or unordered (`v_cmp_lt/le/gt/ge` and `nge/ngt/nle/nlt`) |
| `structure`, `root` | the descriptor is built around a root descriptor's address (which parameter); the first pointer is a constant the program pushes |
| `flags`, `mask`, `instance`, `filter` | first-hit exit (proven from the code, so a constant); culling: none recognised; mask: untested (one tree, no instance); instance code: none; filter: none. The fields exist; only these values are produced |
| `kept` | what the code after the loop reads, each with its form and what it is when no candidate was accepted: distance as numerator times the reciprocal of the denominator; barycentric numerator 0 or 1 times the same reciprocal; facing as one of two constants by the denominator's sign; a constant flag; the node pointer. Distance as a pair or divided, triangle-id bits and instance data have no kind: a loop that keeps them is refused |
| `order` | none (only flags kept); tie by visit (`t` against the running minimum, everything replaced at once); tie by key (equal `t` settled by the smaller pointer). Visit order is a refusal |
| `stack` | bytes per thread; never checked (a cap is a refusal, as is a cap on iterations) |
| `class` | FNV-1a (64 bits) of `forms()`: a line of the forms above and nothing else. **Not in it**: any value or register; the root parameter's index, the root pointer, the stack's stride, the constants of a facing or a flag, and whether a miss leaves a constant or a value. The two-pass program's inner loop (whose misses are values) and the plain walk are one class |

## What is accepted

One shape (`wl/recognise.rs`), checked as truth tables: the lane's
compares are independent cases (at most 2^14), and what the loop keeps,
pushes and stores is followed through every combination of them.

1. One entry, one block that goes round again, one exit; no loop inside;
   one `image_bvh_intersect_ray`, 32-bit pointer, a register per operand.
2. The descriptor built before the loop around a root descriptor's
   address; origin and direction set before it; inverse proven.
3. The pointer popped from group-shared memory at `thread id * stride +
   stack pointer - 4`, in a one-dimensional group; the root a constant
   stored at the thread's base on the one path in, by every lane that
   comes that way (a store under a guard of its own is refused: the lanes
   it leaves out would pop what nothing stored); the loop goes round
   exactly while the stack pointer is not 0.
4. `pointer & 7 < 4` is a candidate; any other pointer pushes every result
   word that is not `0xffffffff`, each once, one above the other; a
   triangle's round pushes nothing, or empties the stack exactly when its
   candidate is accepted (first hit). Nothing else is stored or loaded.
5. Accepted when `minimum < t` and `t < extent`, or also at equal `t` with
   the smaller pointer; everything kept is replaced by the same table.
6. Every value read outside the loop, or handed to its next entry, is one
   kept value as a whole; no lane mask of the loop is used outside it. A
   kept pointer reaches no address outside the loop: straight, through a
   join there (the next pass of a loop around this one), or through an
   instruction the terms have no rule for (an add with a carry out).

**A candidate is not a hit.** The node test answers a triangle the ray
misses with (+inf, 1, 0, 0), so the loop's candidate is then `t` = +inf.
`t < extent` never passes it. `t <= extent` passes it while the extent is
+inf, and "not `t >= extent`" while the extent is a NaN: the walk then
keeps a distance of +inf, weights of 0, the front face's constant and the
missed triangle's pointer as if for a hit, and a ray that hits nothing
ends with that, or with what `kept` says a miss leaves, by whether it came
to test any triangle, which is the guest's box culling. `miss_word` says
which of the two extents the accept passes it at, and is a form of the
class; `Walk::NearestInclusive` and `Walk::NearestUnordered` are such
loops. Shown on the translated walks against `victory-rt`'s traversal
model, which skips a miss: over 8 trees and 384 rays with an infinite
extent, 33 rays that hit nothing store such a "hit" in the inclusive walk
(exactly those on which the model tested a triangle), the same 33 in the
unordered walk when the extent is a NaN, none in the strict ordered walk;
an ordered compare with a NaN extent stores a miss for every ray. What
this means for a host path is in dxr-compatibility.md ("Extents",
"Illegal rays"): no host structure reproduces it.

## Refusals (`Refusal`, 21; a synthetic program for each)

| Reason | Program (`Walk`) |
|---|---|
| no node instruction in the loop | `NoNode` |
| more than one node instruction | `TwoTests` |
| 64-bit pointers or 16-bit directions | `PackedDirections` (not translated) |
| whole-wave walk: the pointer is a move from a scalar register | `WaveWalk` (64 lanes, hit masks kept as numbers; not translated) |
| no per-lane translation (structure, resources, I/O or lowering refused) | `LaneNumber` (a select by a constant lane mask; not translated) |
| a loop inside the loop | `TwoPasses`, outer loop (its inner loop is recognised) |
| the structure's base is not a root parameter's address | `RawBase` |
| origin, direction or extent changes in the loop | `MovingRay` |
| inverse not proven 1/direction | `LoadedInverse` |
| not the known walk (stack, pushes) | `SkipsChild` |
| a store in the loop other than a push | `StoresInLoop` |
| a load in the loop other than the node test and the pop | `LoadsInLoop` |
| a cap on iterations | `Capped` |
| the loop does not end on an empty stack alone | `EarlyExit` |
| more compares decide the loop's values than are enumerated (14) | `Variation::ManyCompares` |
| a compare of an unrecognised form decides a candidate | `BitsCompare` |
| the accepting compares joined in an unrecognised way | `AcceptEither` |
| no compare of `t` against the extent the node test got | `NoExtent` |
| the result depends on visit order | `LastHit` |
| a kept node pointer is loaded through | `NearestIds` (the probe's software walk); `Variation::LoadAtPass`, `LoadWithCarry` |
| a value the loop carries or hands on has no recipe | `Counting` |

Accepted: `Nearest`, `NearestInclusive`, `NearestUnordered`, `Any` (yes or
no), `NearestNode`, `TieByKey`, and the inner loop of `TwoPasses`: six
classes. A loop with several faults gets the first in the order of the
checks. Stricter than the meaning needs, where a title may differ: no
minimum compare, a distance kept alone (classed tie by visit, not none),
a stale temporary a guarded write could keep, a second traversal sharing
the group's memory, more than 14 compares that are not decided on the
way in: each is refused or classed more strictly.

**One reason stands for several conditions**, so the 26 programs above
test one condition of each reason. `wl/walk.rs` therefore holds 25
**variations** (`Variation`): a recognised walk changed in one place so
that one condition fails and a description would be false (one,
`DescriptorInLoop`, writes the descriptor word it had: its container is
`Nearest`'s own and its refusal is caution). Each is translated by the
normal lowering and refused for the reason of the check it breaks; none
is run.

| Condition | Variation | Refused as |
|---|---|---|
| the root is pushed by every lane | `GuardedRoot` | not the known walk |
| the root is pushed on the one path in | `BranchedRoot` | not the known walk |
| the root is stored at the thread's base | `RootAboveBase` | not the known walk |
| the stack pointer starts one entry up | `DeepStart` | not the known walk |
| a one-dimensional group | `Grid` | not the known walk |
| the base is the thread's id along x | `OtherAxis` | not the known walk |
| the node tested is the pointer popped | `NextNode` | not the known walk |
| a first-hit walk empties its stack exactly on acceptance | `ClearsOnMinimum` | not the known walk |
| a push runs exactly for its own valid result word | `WrongChild` | not the known walk |
| pushes go one above the other from the popped entry | `PushesAbove` | not the known walk |
| a box round raises the stack pointer by its pushes | `PushWithoutRaise` | not the known walk |
| a triangle round leaves the stack pointer | `TriangleRaises` | not the known walk |
| the loop tests the round's final stack pointer | `TestsEarlySp` | does not end on an empty stack alone |
| one way out | `SecondExit` | does not end on an empty stack alone |
| the descriptor is not written in the loop | `DescriptorInLoop` | the structure's base |
| a shrinking extent is the kept distance | `ExtentIsWeight` | the ray changes |
| the minimum does not change in the loop | `MinimumMoves` | a compare of an unrecognised form |
| everything kept is replaced by one table | `WeightOnFront` | compares joined in an unrecognised way |
| a tie by key is settled against the kept pointer | `KeyIsFacing` | compares joined in an unrecognised way |
| facing is `0 < d` | `FacingInclusive` | no recipe |
| nothing but kept values is read after the loop | `LastPointer` | no recipe |
| no lane mask of the loop decides anything after it | `MaskedStore` | no recipe |
| a kept pointer reaches no address through a join | `LoadAtPass` | kept and loaded through |
| nor through an instruction the terms have no rule for | `LoadWithCarry` | kept and loaded through |
| at most 14 free compares | `ManyCompares` | more compares than are enumerated |

`GuardedRoot` and `LoadAtPass` were accepted until a review found them,
and `LoadWithCarry` until the correction looked for more of that kind;
all three are now refused. **Whether these tests can fail** was checked
once, outside the tree: 34 checks (of the recogniser, the analyses, the
counters and the census) were switched off in a copy, one at a time, and
the tests run. 33 made a test fail. One did not: the check that no lane
mask of the loop guards an instruction or decides a branch after it.
With it off, `MaskedStore` and every other program written here is still
refused by the check on what is read after the loop: in every program
that could be written here a mask reaches the code after the loop only
through a scalar instruction that reads it. The mask check stays, as a
second barrier no test here tells apart. Beside the variations: a term is a function of everything its
instruction reads (`v_movrels_b32` under two values of M0 is two
things), and a value that leaves a loop only through a phi of the block
after it is live out (`ir:` tests).

## The programs as the software path

`wl/asm.rs` assembles GFX10 words by AMD's names (each form checked
against the decoder). Every program but three translates through the
normal lowering, the node instruction emulated as today. In the
test-only interpreter (`emit/walk_tests.rs`) each of the 22 translated
walks is run one thread at a time over 8 trees `victory-rt` builds (24
or 96 rays a tree), and what a thread stores equals, word for word, a
specification of the walk written from `victory-rt`'s node test. That
specification is the walk's own accept written again, so it shows the
translation, not that an accepted candidate is a hit: that is the test
of the three nearest-hit walks against the traversal model above.

**`workload::walk::software()`** returns the container and bindings of
`NearestIds` for the probe: the nearest hit of each ray in **one bottom
level** of a `victory-rt` image, with geometry and primitive index. Its
hit records equal `victory-rt`'s traversal bit for bit over 8 trees and
1,280 rays (842 hits) when the model forms its quotients as a console
program does (`traverse::Quotient::Reciprocal`, `tn * (1/d)`; the default
division gives another last bit on 364 of those rays). **Missing against
the model**: the top level and instance records (no transform, mask,
instance index or id), the ray's flags, the opaque bit and alpha test,
and every refusal of the model: walking instances in hand-written GFX10
was out of proportion for this item.

## Work, as counts

Two counts, both in `Report` and in the census; neither is a time, and
**no time was measured**: nothing is said of speed.

* `visited`: instructions looked at, each as often as a pass looked at
  it: once for a program with no node instruction; otherwise at most
  `instructions * (2 + 4 * loops)`, asserted for every program.
* `cases` and `evaluated`: the tables. A loop that gets as far has
  2^(free compares) cases, at most 2^14 (a loop with more is refused
  before any table is made), and `evaluated` counts the conditions
  evaluated in them. This is the larger part of the work, and the cases
  **double with each free compare**: the limit on those bounds it, the
  instruction count does not. Asserted: at most 2^14 cases a loop, none
  for a loop refused earlier, and the figures below.

| Program | Instructions | Visited | Free compares | Cases | Conditions evaluated |
|---|---|---|---|---|---|
| `NoNode` | 12 | 12 | - | 0 | 0 |
| `Nearest` | 93 | 465 | 9 | 512 | 9,408 |
| `TwoPasses` (two loops, one refused before its tables) | 97 | 639 | 9 | 512 | 9,408 |
| `TieByKey` | 99 | 539 | 11 | 2,048 | 40,768 |

Memory: the tables of one call (terms, cases), freed when it returns;
nothing long-lived, so no row in [memory.md](memory.md).

## Where the class goes (joined to nothing yet)

The class is the key the validation record and `select()` are keyed by
(`crates/victory-gpu/src/rt/record.rs`, `rt/select.rs`;
[dxr-host-backend.md](dxr-host-backend.md), "Selection"). There it is
opaque: compared for equality, never a title shader's name or hash, which
holds of a hash of forms. **Nothing joins the two in code**: no generator
exists, so nothing fills a `select::Workload`.

When a generator exists, the hash reaches the record this way: the
translator that recognised the region and made its variants fills
`select::Workload::class` from `Description::class`; the probe and the
title comparison write their rows under the same value
(`record::Key::class`); and the class a census line prints is then what
`VICTORY_RT_CLASS` narrows a forced path by.

**The two agree in form and not in size**, found when the two changes met.
Both are a hash written as lowercase hexadecimal digits. `describe::Class`
is 64 bits (FNV-1a), 16 digits. `record::Class` is 32 bytes, 64 digits;
its `short()`, and the least `VICTORY_RT_CLASS` takes, are the first 16.
So the value made here cannot be handed over as it is, and a class in a
census is not yet something a record row or `VICTORY_RT_CLASS` can be
given. Whoever joins them decides how 8 bytes become 32 (a 32-byte hash of
the same `forms()` text made here, or the 8 bytes placed first in the key
by a fixed rule); either changes class values only, and nothing stores one
yet. And the class leaves out what the table above says it does (the root
parameter's index, the stride, constants): a row keyed by it must carry
what of those its generator depends on (dxr-architecture.md, "The layer").

## Defaults in force

The coordinator's, at integration, until the owner says otherwise:

* The census switch is named `--rt-census`.
* A loop whose accept passes the emulated miss word is **recognised and
  marked** (`miss_word`, a form of its class), not refused. What that
  means for the paths (dxr-compatibility.md, "Extents", "Illegal rays")
  is a proposal the owner has not seen.
* A kept node pointer that is loaded through, and a loop with a cap on
  iterations or on its stack, are **refused**: software.

## The census

`victory translate <folder>` adds to its report two lines that begin
`ray tracing` (the counts; the recogniser's work), a line per recognised
class (count, class hash, forms) and a line per refusal reason (count),
from `workload::Census`. A loop counts when it holds a node instruction;
a loop around another counts too (as "a loop inside the loop"). A
compute program whose IR was not built is not examined and is counted as
such in the first line; a container whose programs cannot be listed at
all is in the report's failure list only.

**Those lines hold no program name, file, offset, shader hash or code;
the rest of the report does** (the `--list` lines and the first example
of each failure name the file and the pack's record). So the census has
a switch of its own, and only its output is meant to leave the machine:

```
victory translate <shader folder> --rt-census > rt-census.txt
```

prints the census lines alone on standard output (progress, which names
files, goes to standard error and not into the file). The call site and
the switch are type-checked for Windows only and have never run.

What a census on the owner's machine would settle:

* whether any title loop has this shape at all, and which refusals
  dominate: the next shapes to write are the most frequent, not guesses;
* how many tracers are whole-wave walks or have no per-lane translation;
* how many keep a node pointer and load through it (side tables before
  native), have a cap (software by the owner's default), or load inside
  the loop (an alpha test or an instance walk: the filter and instance
  fields have no recogniser yet);
* how many recognised loops accept a missed triangle at an infinite or
  NaN extent (the `miss-word` form of their class);
* how many node instructions sit in no loop (not covered by the design).

It settles nothing about equivalence or speed: a class it finds still
needs a probe program and a title comparison (dxr-validation.md).
