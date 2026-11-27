# BCPack: the two lane corrections against the decoder before them

Measured on 2026-11-27 for the owner's decision of 2026-11-26 ([progress.md](progress.md), session
log): adopt both corrections "if validation establishes that they correctly handle valid synthetic
streams", as a separate, reversible commit with their regression tests re-enabled, compared with the
existing code in accuracy, throughput, memory and error handling; no regressions; agreement between
related implementations is not independent proof. As this is written the corrections are a patch
applied to the working tree of `wip/bcpack` at `ae1c915`, not yet a commit. Every statement below
says which of these it rests on:

* **old**: `crates/victory-formats/src/bcpack.rs` at `ae1c915`; **new**: the same file with the two
  corrections.
* **the port**: the vendored part of `bcpack-rs` (`crates/bcpack-reference`). Its lane is a port of
  Microsoft's module; its feeder, which decides how lanes share one byte stream, is that project's
  own. Old, new and the port are **related**: agreement among them proves nothing by itself.
* **the module**: `decoder` of Microsoft's `mdu_minirans.v`, simulated unmodified. The only
  independent authority, and it is **one lane**. How lanes share a stream is not published.

## The two corrections

Both are in `Lane::step` and `lanes` of `crates/victory-formats/src/bcpack.rs`.

1. **A symbol count per lane** (`Lane::owed`). A lane that has decoded its symbols reads what is
   left of the fragment it is in and takes no more input. Old's lane asked for its next header
   whether or not it owed symbols.
2. **The kept byte and the later end.** A fragment without the reset bit that ends on a read hands
   on the state with that byte (old: the state before it); and a fragment whose last byte leaves the
   state below 2^24 ends in the next step, not the same one (with the reset bit too).

With them the nine tests ignored for this reason run, the two `known_difference_...` tests of
`bcpack/differential.rs` give way to the regression tests beside them, and the test writer writes no
filler fragment. The block stage and everything before the lane code are unchanged text.

## How it was measured

* Toolchain 1.98.1, release builds with the workspace's release profile, in which arithmetic
  overflow wraps; the accuracy and error runs were repeated in a build with overflow checks and
  debug assertions on and printed the same, line for line. A Linux container, 4 cores, 15 GB,
  **shared with other work** (load average 1.8 to 6.9 when the timings started). Nothing ran on
  Windows.
* One scratch program, `lane-compare`, kept outside the repository (its place is in the session's
  report), built with `cargo build --release`. It links old and new side by side, each the source
  file byte for byte (`cmp`) with one probe module appended, and the port. Tables name the
  subcommand that printed them; `accuracy2` and `errors2` were added after a review of the first
  version of this document, and the other subcommands print what they printed before.
* Every stream is synthetic and made by that program; no game data. Its self-check: old's writer
  shape reproduces `FILLER_STREAM` of `bcpack.rs` byte for byte; its plain shape equals the port's
  encoder on 60 of 60 streams; the two regression streams are refused by old, decoded by new.
  Block-stage constants are made up; the models are Microsoft's. "written" = the symbols a stream
  was encoded from.

## Tests with the corrections

`cargo test -p victory-formats -p bcpack-reference`: 69 passed, 0 failed, 5 ignored. `cargo xtask
policy`: ok. `scripts/check-linux.sh windows` (tier 2): ok, both feature sets; type-checked, not
linked. `scripts/check-linux.sh tests` (tier 1): 519 passed, 0 failed, 9 ignored, in two of the last
three runs; the third failed in one wall-clock test of another crate
(`frame::tests::waits_end_on_ticks_and_count_frames` of `victory-gpu`), which on this loaded machine
also fails without the patch (`cargo test -p victory-gpu --lib`, 20 runs each: 1 failure in a copy
of `ae1c915`, 2 with the patch). None is ignored for the lane reason. Ignored still: benchmarks
(`bcpack_throughput`, `imdct_throughput` and four of other crates) and three tests that need files
outside the repository (`reference_chunks`, `matches_a_reference_decoder`, `walks_saved_streams`).

## Accuracy 1: one lane against Microsoft's module

**Rests on the module: the only independent evidence, and it covers one lane.** Not rerun here;
cited from the oracle's report (scratch, `lane-oracle/RESULTS.md` as assembled on 2026-11-27 at
01:31 UTC after its review, unchanged when this was finished). Its (a) is `bcpack.rs` at `b349d72`,
whose lane code differs from old's only in the name of a parameter type and which read its models
from the table file (equal to those of `bcpack/models.rs` on the entries the decoder uses, by the
note in that file; not rechecked here); its (b) is a file whose code outside the tests is byte for
byte new's (`diff`). Four seeds, the report's own encoder.

* **Correction 1.** Within one run (one `go`) the module delivers `out_target` symbols and no more,
  reads no further header once they are out, finishes its fragment and goes idle. New equals it up
  to that point in symbols, bytes and requests on 42,000 of 42,000 valid streams (189,000 of 189,000
  over four seeds). Old's lane asks for a further header after its last symbol on every stream.
* **Correction 1, the bytes behind a finished lane: conditional.** The text does not say how long
  `go` is held, and what drives it is not published. `go` low when the lane goes idle: the module
  takes none of them (9,600 of 9,600), nor does new (9,600 of 9,600); old takes them (5,385 of 5,385
  where nothing else had gone wrong). `go` held high: the module starts a new counted run on them
  from a cleared state, and neither version is the module (new on 0 of 9,600, old's lane on 3,241).
  The report: confirmed "on this condition: that one call of `lanes` stands for one run of the
  module (one go) and that go is low when a lane finishes".
* **Correction 2, the kept byte: confirmed by decoded symbols.** New returns the module's symbols on
  all 16,000 streams with such fragment ends; old on 2,401, failing or returning wrong symbols on
  13,599.
* **Correction 2, the later end: "the module's step order, one lane; its need in a group not
  established".** The module's lane waits a step before asking for the next header and so does new
  (42,000 valid streams, 29,400 held runs, under the report's own mapping of steps). On one lane the
  wait changes no symbol and no byte (a variant without it: equal on 8,000 of 8,000).
* **Regressions, one lane.** Valid streams: none found; in 189,000 there is none on which old
  returns the module's symbols and new does not (old is right on 109,348). Invalid input: new more
  often returns success where the module stalls or flags the stream (84 of 10,092 truncated streams
  where old had matched the stalled module), because old had already failed there.
* **Limits.** One lane. "Valid" means written by the report's encoder and finished cleanly by the
  module; streams in old's conventions (fillers) are outside it. Two readings of `go`. One two-state
  simulator. The published file includes a header the repository does not ship.

## Accuracy 2: streams in the port's shape and byte order

**Rests on the port, a related implementation: this is agreement among related implementations, not
proof.** Every stream has the byte order the port's feeder asks for. `lane-compare accuracy 5` (sets
A to C: 14,760 streams of 1 to 4,096 blocks, 32,420,262 fragments, all 128 models) and `lane-compare
accuracy2 4` (sets D to F: 5,424 streams).

A = flushed fragments, the port's encoder (every model and narrowing, 1 to 2,000 symbols a fragment,
9,003 3-byte headers). B = last bytes kept, or the state handed on by fragments ending with the read
that decodes their last symbol. C = the state handed on by fragments that may end inside a read (B
and C: at most 80 symbols a fragment, no 3-byte header). D = as B and C with up to 40,000 symbols a
fragment (7,315 3-byte headers without the reset bit; the longest fragment 39,189 bytes). E = each
rANS stream under one model, cut at byte positions that have nothing to do with symbols, and lanes
of mixed kinds (1 to 400 blocks; 397,314 one-byte fragments; 101,990 fragments in which no symbol is
decoded). F = fragments of exactly 65,792 bytes, the most a header can say (785 of them; 938 3-byte
headers without the reset bit; 150,000 or 300,000 symbols a lane).

Each cell: streams: old written / other symbols / refuses. **New and the port return the written
symbols on every stream of every cell** (20,184 of 20,184).

| format | A | B | C | D | E | F |
|---|---|---|---|---|---|---|
| BC1 | 1020: 521 / 0 / 499 | 960: 277 / 0 / 683 | 480: 114 / 103 / 263 | 128: 122 / 6 / 0 | 768: 8 / 480 / 280 | 8: 4 / 4 / 0 |
| BC3 | 1020: 577 / 0 / 443 | 960: 339 / 0 / 621 | 480: 150 / 124 / 206 | 128: 122 / 3 / 3 | 768: 2 / 661 / 105 | 8: 8 / 0 / 0 |
| BC4 | 1020: 510 / 0 / 510 | 960: 281 / 0 / 679 | 480: 112 / 99 / 269 | 128: 125 / 2 / 1 | 768: 5 / 474 / 289 | 8: 7 / 0 / 1 |
| BC5 | 1020: 576 / 0 / 444 | 960: 326 / 0 / 634 | 480: 155 / 105 / 220 | 128: 123 / 4 / 1 | 768: 6 / 657 / 105 | 8: 6 / 2 / 0 |
| BC6H | 1020: 562 / 0 / 458 | 960: 328 / 0 / 632 | 480: 136 / 159 / 185 | 128: 112 / 9 / 7 | 768: 2 / 664 / 102 | 8: 7 / 1 / 0 |
| BC7 | 1020: 540 / 0 / 480 | 960: 332 / 0 / 628 | 480: 129 / 169 / 182 | 128: 114 / 8 / 6 | 768: 2 / 666 / 100 | 8: 6 / 2 / 0 |
| **all six** | 6120: 3286 / 0 / 2834 | 5760: 1883 / 0 / 3877 | 2880: 796 / 759 / 1325 | 768: 718 / 32 / 18 | 4608: 25 / 3602 / 981 | 48: 38 / 9 / 1 |

* Old returns the written symbols on 6,746, refuses 9,036 and returns other symbols with no error on
  4,402. Pairwise: new = port on 20,184; old = port and old = new on those 6,746 and on no other.
  There is no stream on which old returns the written symbols and new does not. The port's two
  decoders agree on every stream of at most 64 blocks.
* Whole `decode`: where old's and new's records agree, so do their results and blocks (no
  exception). BC1 blocks of new equal the port's rebuild on 3,364 of 3,364; the port has no other
  block stage here.
* Each correction alone (scratch variants, not the patch; sets A to C only): correction 1 gives the
  written symbols on 11,880 of 11,880 of A and B and on 2,038 of 2,880 of C (764 other symbols, 78
  refused); correction 2 leaves A and B as old and gives 956 of C, none wrong, 1,924 refused.
* Not in these sets: a fragment behind a lane's last symbol (not part of the lane's stream by the
  oracle's report), and lanes of more than 300,000 symbols.

## Accuracy 3: streams in old's own byte order

**Rests on old's own conventions.** Old's lanes went on asking after their last symbol, so a stream
old decodes ends each lane with a filler fragment that parks it, placed where old's lane asks. These
streams are the lanes' fragments and a filler each, interleaved in the order **old** takes the
bytes: the cases where new changes what old did. `lane-compare oldorder 5`: 9,600 made, 9,224
accepted by old (9,188 with the written symbols), 1 to 300 blocks.

B1 = old's test writer (one fragment a lane, model 0, no reset bit, last bytes kept; filler `00 01
00 00`). B2 = several flushed fragments a lane (filler `00 04 01 00 00 00 00`). B3 = the state
handed on, ending with the read that decodes. B4 = B3 with the last fragment without the reset bit.
B5 = the state handed on, ending inside a read, where old's rule and the module's exclude each
other. Each cell: streams old accepts: new same symbols / other symbols / refuses.

| format | B1 | B2 | B3 | B4 | B5 |
|---|---|---|---|---|---|
| BC1 | 320: 280 / 0 / 40 | 320: 127 / 190 / 3 | 320: 173 / 144 / 3 | 320: 184 / 82 / 54 | 269: 147 / 122 / 0 |
| BC3 | 320: 320 / 0 / 0 | 320: 150 / 166 / 4 | 320: 192 / 125 / 3 | 320: 197 / 95 / 28 | 260: 176 / 78 / 6 |
| BC4 | 320: 320 / 0 / 0 | 320: 141 / 176 / 3 | 320: 182 / 137 / 1 | 320: 183 / 89 / 48 | 268: 154 / 113 / 1 |
| BC5 | 320: 320 / 0 / 0 | 320: 167 / 149 / 4 | 320: 211 / 106 / 3 | 320: 210 / 94 / 16 | 267: 181 / 84 / 2 |
| BC6H | 320: 320 / 0 / 0 | 320: 142 / 93 / 85 | 320: 197 / 109 / 14 | 320: 210 / 90 / 20 | 248: 154 / 83 / 11 |
| BC7 | 320: 320 / 0 / 0 | 320: 156 / 81 / 83 | 320: 228 / 82 / 10 | 320: 226 / 70 / 24 | 232: 169 / 54 / 9 |
| **all six** | 1920: 1880 / 0 / 40 | 1920: 883 / 855 / 182 | 1920: 1183 / 703 / 34 | 1920: 1210 / 520 / 190 | 1544: 981 / 534 / 29 |
| of all six: fillers all behind the data | 1520: 1520 / 0 / 0 | 798: 798 / 0 / 0 | 738: 738 / 0 / 0 | 763: 763 / 0 / 0 | 705: 692 / 10 / 3 |
| of all six: a filler before data | 400: 360 / 0 / 40 | 1122: 85 / 855 / 182 | 1182: 445 / 703 / 34 | 1157: 447 / 520 / 190 | 839: 289 / 524 / 26 |

The last two rows split the "all six" row by where the fillers lie: "behind" = no filler byte is
taken before the last byte of the lanes' own fragments. The port accepts 4,550 of the 9,224, always
with old's symbols.

* A finished lane of new leaves its filler in the stream and the lanes still reading take it. B1:
  the 40 refused are the one-block BC1 streams (40 of 40); all else is the same, in part because a
  filler of zeros and ones reads as empty fragments to the lane that takes it.
* Each correction alone: correction 1 changes 3,080 of the 9,224 (2,632 other symbols, 448 refused),
  none with fillers behind; correction 2 changes 451 (175 and 276: B1 80, B4 335, B5 36), since
  old's filler lengths fit the byte old dropped.

Smallest examples, the bytes in old's order (the last belongs to "Error handling"):

* new refuses: B4 BC1 blocks=1, 96 bytes: `40840206c908898f 0303030303030303 80c01c1301030101 0030eb0707b0b600 6ce68952df392b08 00c1608ca7110701 0d0b06c200000000 0000000001010101 0101010100000000 0000000003030303 020b3a11e0bd83cc 535a2c16c17ca0fa`
* new returns other symbols: B4 BC5 blocks=2, 112 bytes: `0d074b00080b0904 0304040703040405 010554018e010909 01da4c003f8591b9 2065725219993c12 4ca2d2b1ff9b1a1b 23a82374bb800d0a 0304080102020406 392145017429d456 047249102818a593 6e7fc00000aa0008 0000000001000000 0101010133010101 0000000000000000`
* old's test writer, new refuses: BC1 blocks=1, 104 bytes: `0000000000000000 0404030303030303 0101204020204020 0000000000000000 3b354808d0600c98 0000000000000000 0000000000000000 0000000000000101 0101010101010000 0000000000000000 0404040401010101 00000000bf1ee725 0000000000000000`
* fewer blocks asked, new's last block differs: BC4 dst=8, 84 bytes: `1115151f96151c95 0404040303040303 c1201e0103080749 10ec8a0256ea2b70 2f830503159fbcb0 902738028796cf15 7b6710cc1e141994 1703040403030106 01510c0340e8d115 1247ed1e4d1a43d0 9fd02378`

Which byte order real streams have is the unpublished part; neither table settles it.

## Error handling

**Rests on old and new given the same bad input; the port (related) beside them.** `lane-compare
errors 3` and `errors2 3`: `decode` of both on 241,680 inputs (74,192 and 167,488; 648 unchanged
base streams are in both), each also with the shipped constants. **No panic** in either version, in
the release build (overflow wraps) and in the build with overflow checks on. The same five error
texts in both; new adds none.

Base streams. Old and new decode these alike: P = 432 of the port's encoder, F = 216 of old's test
writer with fillers, S = 200 in the port's order whose fragments hand the state on and may end
inside a read. T = 432 like S on which new and the port return the written symbols and old does not
(old accepts 98 with other output and refuses 334). "same result" = both succeed with the same
output or both refuse with the same text.

* **The same in old and new** (34,296 inputs). On P, F and S: unchanged (848); cut at the end (7
  points) or at a random point (14,416: both return the unchanged output on the same 741 and refuse
  the rest, "the stream ends early or is corrupt"); bytes appended (5,136: both return the unchanged
  output); more blocks asked than written (1,696: both refuse, the same text). With no base stream:
  a destination that is empty or not whole blocks (2,592, "destination is not a whole number of
  blocks"); empty input (24); format number 0, 2, 8 or 255 (8, "unsupported BC format"); a record
  selecting an unused BC7 control code (180, "unused BC7 control code") or a BC6H mode out of range
  (180, "BC6H mode out of range"); one byte value repeated (9,216: both accept the same 5,184).
* **Not the same** (the last row of each part also counts its classes that are the same):

| class | base | inputs | old ok | new ok | same result | both ok, other output | old ok, new refuses | old refuses, new ok | both refuse, other text |
|---|---|---|---|---|---|---|---|---|---|
| one bit flipped | P, F, S | 25440 | 19745 | 19952 | 22434 | 2482 | 131 | 338 | 55 |
| one byte deleted | P, F, S | 12720 | 5423 | 5660 | 9772 | 2327 | 149 | 386 | 86 |
| one byte inserted | P, F, S | 12720 | 6789 | 7197 | 9748 | 2215 | 134 | 542 | 81 |
| bytes swapped; a run doubled; a run set to 00 or ff | P, F, S | 38160 | 23000 | 23592 | 30697 | 5866 | 418 | 1010 | 169 |
| the first 1 to 8 bytes removed | P, F, S | 6784 | 3490 | 3530 | 3254 | 2851 | 298 | 338 | 43 |
| decoded under another of the six format numbers | P, F, S | 4240 | 2343 | 2357 | 2978 | 1140 | 47 | 61 | 14 |
| fewer blocks asked than written (one fewer, half) | P, F, S | 1696 | 1696 | 1672 | 1280 | 392 | 24 | 0 | 0 |
| pure noise | none | 10800 | 7726 | 7723 | 10186 | 572 | 22 | 19 | 1 |
| noise of byte values 0 to 3 | none | 10800 | 4082 | 4146 | 6118 | 3751 | 323 | 387 | 221 |
| noise of byte values 0 to 15 | none | 10800 | 6905 | 6897 | 3843 | 6058 | 416 | 408 | 75 |
| noise shaped as short fragments | none | 10800 | 2024 | 2030 | 10158 | 556 | 35 | 41 | 10 |
| **all** | P, F, S, none | 179256 | 95132 | 96665 | 144764 | 28210 | 1997 | 3530 | 755 |
| unchanged | T | 432 | 98 | 432 | 0 | 98 | 0 | 334 | 0 |
| cut at the end (7 points) or at a random point | T | 7344 | 744 | 43 | 6242 | 13 | 731 | 30 | 328 |
| bytes appended (8 zeros, 8 random, the stream again) | T | 1296 | 605 | 1296 | 156 | 449 | 0 | 691 | 0 |
| changed inside or at the head (bit, byte, swap, run, first bytes) | T | 48816 | 20143 | 29245 | 22581 | 13167 | 1623 | 10725 | 720 |
| decoded under another of the six format numbers | T | 2160 | 1221 | 1245 | 1408 | 664 | 28 | 52 | 8 |
| fewer blocks asked than written | T | 864 | 783 | 850 | 457 | 320 | 10 | 77 | 0 |
| more blocks asked than written | T | 864 | 151 | 0 | 652 | 0 | 151 | 0 | 61 |
| **all** | T | 61776 | 23745 | 33111 | 31496 | 14711 | 2543 | 11909 | 1117 |

* **T, cut or over-asked**: old returns success on 744 of 7,344 cut streams (never with the
  unchanged stream's output) and on 151 of 864 calls asking for more blocks than written; new on 43
  (with the unchanged output) and on none.
* **Fewer blocks asked than written** (P, F, S: 1,696 calls): old returns the leading blocks of the
  full output on all. New on 1,280; another output with no error on 392; it refuses 24. Of the 1,296
  calls on P and F: 284 differ, in the last block only (284 of 284), and 21 are refused; correction
  1 alone 283 and 21, correction 2 alone none.
* **A change inside a stream or at its head, another format number, noise**: differences in both
  directions. Neither version checks more than the other; the format has no redundancy.
* **Against the port**: wherever it accepts an input from P, F, S or no base (13,337) both versions
  return its symbols, and for BC1 its blocks (2,198 of 2,198). Of the 2,853 inputs from T it
  accepts, new returns its symbols on all, old on 6. Old succeeds on 105,146 inputs the port
  refuses, new on 114,041.
* **End states**: neither version checks the state a lane ends in. The oracle's report: the module
  raises `error_overrun` when the state after the last symbol is not 2^24, 2^16 or 2^8; VIctory's
  entry point, old and new, returns success with the same symbols on such streams.

## Throughput

**Rests on old and new timed in one binary, on streams of the port's encoder that both decode
alike.** `lane-compare speed 2`: 16,384 blocks, one thread, megabytes (10^6 bytes) of BC output a
second; 18 timings a cell of about 60 ms each, old and new alternating, each decoder into buffers of
its own. "short" = fragments of 1 to 16 symbols; "long" = one fragment a lane. Median
(minimum-maximum); ratio = the median of new/old over the pairs of timings. **A difference of
medians under the larger spread counts as none.** The table is the last of eight runs (load average
5.4 at its start).

| format | fragments | entropy stage: old; new; ratio; by the rule | whole `decode`: old; new; ratio; by the rule |
|---|---|---|---|
| BC1 | short | 13.7 (13.4-13.8); 13.1 (12.2-13.2); 0.954; none | 13.0 (12.7-13.1); 12.5 (12.2-12.6); 0.965; slower, 3.8 % |
| BC1 | long | 48.1 (46.9-48.6); 44.5 (43.8-45.1); 0.929; slower, 7.4 % | 43.4 (42.9-43.6); 40.6 (40.2-40.7); 0.935; slower, 6.4 % |
| BC3 | short | 14.3 (12.7-14.4); 13.7 (10.1-13.8); 0.957; none | 13.6 (12.8-13.8); 13.0 (11.1-13.3); 0.964; none |
| BC3 | long | 65.2 (61.5-66.1); 60.9 (56.5-61.3); 0.930; none | 58.1 (48.9-59.5); 55.5 (36.5-56.1); 0.951; none |
| BC4 | short | 13.8 (11.5-13.9); 13.3 (10.8-13.4); 0.960; none | 13.2 (11.5-13.2); 12.7 (11.6-12.8); 0.966; none |
| BC4 | long | 42.2 (40.2-42.4); 39.6 (38.6-39.7); 0.940; slower, 6.2 % | 38.1 (36.0-38.4); 35.6 (33.3-36.0); 0.937; none |
| BC5 | short | 14.4 (14.3-14.5); 13.8 (13.6-13.9); 0.961; slower, 3.9 % | 13.8 (13.7-13.9); 13.3 (12.9-13.4); 0.964; slower, 3.7 % |
| BC5 | long | 59.5 (54.7-59.8); 55.5 (51.2-55.9); 0.931; none | 54.7 (53.9-55.1); 50.9 (50.6-51.3); 0.931; slower, 7.0 % |
| BC6H | short | 10.0 (9.9-10.1); 9.7 (9.5-9.7); 0.963; slower, 3.8 % | 8.8 (8.5-8.8); 8.5 (8.0-8.6); 0.972; none |
| BC6H | long | 35.7 (34.0-36.0); 33.0 (29.2-33.4); 0.923; none | 25.1 (24.4-25.3); 23.8 (21.9-24.0); 0.946; none |
| BC7 | short | 9.7 (9.2-9.8); 9.3 (8.6-9.4); 0.960; none | 8.7 (8.4-8.8); 8.4 (7.6-8.5); 0.969; none |
| BC7 | long | 25.7 (22.5-26.0); 24.3 (22.0-24.7); 0.952; none | 20.5 (17.0-20.7); 19.6 (12.8-19.8); 0.952; none |

* By the rule: no difference in 16 of these 24 cells, new slower in 8. Over the eight runs (load
  average 1.8 to 6.9): none in 147 of 192 cells, new slower in 45 (2, 10, 14, 8, then 0, 1, 2, 8),
  new faster in none. The ratio is below 1 in 190 of the 192 cells (0.913 to 0.999; the other two
  1.003 and 1.017, in one run): one direction.
* **The size is not a figure.** It lies between 0 and 9 % and moves with the build of the program.
  The first run was made by an earlier build (`speed 1`, 9 timings) and gave 0.979 to 0.999 for the
  entropy stage on short fragments; the seven later runs, of three later builds that differ outside
  the two decoder files, give 0.945 to 0.972. A reviewer's harness of the same two decoders moved by
  about 3 points between two builds (whole `decode`, short fragments: 0.990 to 1.001, then 0.917 to
  0.972). Whether the build, the shorter run or the load made the first run differ was not
  separated.
* Scratch variants, not the patch: with the count in 32 bits (a lane of 24 bytes) the ratio was 0.93
  to 0.98, so the larger lane is not the whole of it; the two corrections were not timed apart on a
  quiet machine.

`cargo test --release -p victory-formats bcpack_throughput -- --ignored --nocapture` (the
repository's benchmark: synthetic, one fragment a lane), two sets of six runs of each tree,
alternating; megabytes a second, median (minimum-maximum) of six; the new tree's writer writes no
fillers:

| format | first set: old tree; new tree; difference of medians; larger spread; by the rule | second set: the same |
|---|---|---|
| BC1 | 57.8 (50.9-58.1); 54.5 (32.1-55.2); -3.3 (-5.7 %); 23.1; none | 53.8 (49.7-58.2); 53.0 (35.8-55.4); -0.8 (-1.6 %); 19.6; none |
| BC4 | 55.9 (55.4-56.7); 53.2 (50.0-53.7); -2.7 (-4.9 %); 3.7; none | 55.8 (46.5-56.8); 53.5 (50.2-53.7); -2.3 (-4.2 %); 10.3; none |
| BC7 | 32.4 (28.4-32.7); 30.9 (29.9-31.4); -1.5 (-4.6 %); 4.3; none | 31.6 (29.9-32.7); 30.9 (29.2-31.3); -0.7 (-2.1 %); 2.8; none |

By the rule none in all six cells; the new tree's median is the lower in all six (1.6 to 5.7 %).

## Memory

**Rests on a counting allocator in the scratch program.** `lane-compare memory`: 108 rows (six
formats, 2 to 65,536 blocks; a call that succeeds, one that fails in the entropy stage, `lanes`
alone), **every one equal in old and new**; nothing is left allocated.

| format | blocks | output bytes | `decode` succeeds: allocations / bytes / peak live | `decode` fails in the entropy stage: allocations / bytes / peak live | `lanes` alone | old = new |
|---|---|---|---|---|---|---|
| BC1 | 2 | 16 | 2 / 10264 / 10264 | 1 / 24 / 24 | 0 / 0 / 0 | yes |
| BC1 | 65536 | 524288 | 2 / 796672 / 796672 | 1 / 786432 / 786432 | 0 / 0 / 0 | yes |
| BC7 | 2 | 32 | 2 / 29760 / 29760 | 1 / 64 / 64 | 0 / 0 / 0 | yes |
| BC7 | 65536 | 1048576 | 2 / 2126848 / 2126848 | 1 / 2097152 / 2097152 | 0 / 0 / 0 | yes |

The heap is the record buffer and the block stage's history. The lane state grows: `Lane` from 20
bytes to 32, the bank of 16 on the stack of `lanes` from 320 to 512. Stack depth was not measured.

## Where new differs from old on a stream old accepted

1. Port-order streams old accepted with other symbols than written (4,402 of the 11,148 it accepts;
   sets C to F): new returns the written ones.
2. Old-order streams with fillers (3,087 of 9,224): other symbols 2,612, refused 475; all but 13
   have a filler before data.
3. Fewer blocks asked than written (416 of 1,696 on P, F, S): another output 392, refused 24.
4. Streams that hand the state on inside a read and that old did not decode as written (T), cut
   short or asked for more blocks: old returned success on 744 and 151; new refuses 731 and 151 of
   those.
5. Changed streams and noise: other blocks or a refusal, in both directions, as counted above.
6. Speed (a direction, not a figure) and stack, as above.

## Not checked

* How several lanes share one stream: not published. Nothing here is independent evidence for either
  byte order.
* No real stream was used: whether real streams have fillers, fragments without the reset bit or
  ending inside a read, or lanes that finish at different times is not known. Nor whether the
  runtime ever asks for fewer blocks than a stream holds.
* Nothing ran on Windows. No timing on an idle machine or another one.
* The block stages of BC3 to BC7 against anything from outside.
* The oracle was not rerun here. Sets D to F and the base streams S and T were not run with one
  correction alone.

## Settled at the commit

The patch's own text said more than the oracle's reviewed conclusion (`THIRD_PARTY_NOTICES.md`: two
rules "were corrected ... to what the Verilog does"; the header of `bcpack/differential.rs`: "the
Verilog decided"). Both now say what the module settles and point here. `docs/progress.md` no
longer describes the corrections as held back.

## Undoing it

The three files (`bcpack.rs`, `bcpack/differential.rs`, `THIRD_PARTY_NOTICES.md`) are one commit;
this document and the `progress.md` entry are another. `git revert` of the first restores old's
decoder, its test writer, the nine `#[ignore]`s and the two `known_difference_...` tests.

## Verdict

**Correction 1 (the symbol count).** *Valid = one lane's stream as Microsoft's module decodes it:*
established for the lane's own symbols and bytes; for the bytes behind a finished lane, only on the
oracle's condition (one run of the module per call, `go` low when the lane finishes). *Valid = a
stream in the port's byte order:* every stream tried is handled (11,880 of 11,880 of A and B with
this correction alone); agreement with a related implementation. *Valid = a stream in old's byte
order with fillers:* not established; this is what it changes. *Regressions found:* old-order
streams with a filler before data (3,080 of 9,224 with this correction alone); calls asking for
fewer blocks than written (416 of 1,696 with both corrections; on P and F 305 of 1,296, 304 with
this one alone, none with the other).

**Correction 2 (the kept byte and the later end).** *Valid = one lane's stream as Microsoft's module
decodes it:* the kept byte is established by decoded symbols; the later end is the module's step
order on one lane, and its need among several lanes is not established. *Valid = a stream in the
port's byte order:* every stream tried is handled (2,880 of 2,880 of C and 5,424 of 5,424 of D to F
with both corrections; 2,038 of C without this one); agreement with a related implementation. *Valid
= a stream in old's byte order with fillers:* not established. *Regressions found:* old-order
streams with fillers (451 of 9,224 with this correction alone).

**Both together, not separated by correction.** On the 4,402 port-order streams of item 1 new
replaces old's symbols by the written ones, and on cut or over-asked streams of kind T it refuses
what old accepted (item 4); on the cut streams old's output was never the unchanged stream's.
*Throughput:* by the stated rule no difference in the repository's benchmark (3 of 3 formats, twice)
and in 147 of 192 cells of the scratch program; new is faster in no cell and the paired ratio
new/old is below 1 in 190 of 192. The size is 0 to 9 % and moves with the build, so this is a
direction on this shared machine, not a figure.
