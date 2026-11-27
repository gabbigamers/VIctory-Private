# Predictive world streaming

What will be needed: the title's own streaming requests observed, future
demand anticipated from them, and converted results kept, with the title's
request staying the only thing that decides what is loaded. The shared
notions are in [resource-model.md](resource-model.md); the bounds it is
reclaimed under are memory management's
([unified-memory.md](unified-memory.md)).

**Nothing here was run, and no title request has been seen by this work.**
Standing of each statement: a file and line in backticks means *read from
the code*; **doc** names the document that reports it; **inferred** says
from what; **unknown** names the run on the owner's machine that would
settle it; **proposed** is this page's design, none of it implemented,
tested or measured. `ds:` is `crates/victory-hle/src/modules/dstorage.rs`;
`gpu/`, `hle/`, `pipe/` as in resource-model.md. No gain is stated.

## What a streaming request is today

| Fact | Where |
|---|---|
| A 0x48-byte record the title enqueues: options (zlib bit, a BC number, page destination, memory source), destination, sizes, source, offset, tag, name | `ds:694-727` |
| One pool of workers, one per logical processor, serves every queue; three classes weighted 100, 10, 1 by bytes; at most 32 MiB of compressed source in flight | `ds:333-337, 430-442, 510-530` |
| A request has no deadline; a started read runs to its end; cancel by tag is a missing slot | `ds:510-530, 1146` |
| Status entries complete in queue order with the first failure; the title polls them | `ds:637-655` |
| A page destination's page list is valid only from the title's own Submit | `ds:562-568` |
| A memory source is title memory read when the request executes | `ds:998-1004` |
| Files are opened with flags 0 and read by synchronous buffered reads | `ds:124-134, 1031` |
| The decode (zlib, BCPack, or both) happens at request time, into title memory, announced to the change tracker | `ds:873-911, 1049-1058`; `hle/xmem.rs:514-522` |
| A second way in, the title's own `ReadFile`, shows a buffer and a length only | `hle/files.rs:531-542` |
| A host texture is made at the title's first use; detile and upload run before the first submission that uses it; detiled bytes are not kept | `gpu/resource.rs:781-783`; `gpu/texture.rs:590-596` |

BC data is not transcoded: the only texture conversion is the detile.
Nothing of a request survives its completion, and the only per-request
line is at trace level without file, queue or source size (`ds:912-917`).

## Observation first: the request log

Behind `VICTORY_STREAM_LOG=1` (**proposed**), written under the cache
folder, which is outside the repository by construction
(`pipe/cache.rs:37-41`). One row per request, with the fields the layer
sees and nothing of the title's content:

| Field | From |
|---|---|
| Sequence over all queues | the scheduler's own (`ds:342-343`) |
| Queue number and class | ours (`ds:333-335`) |
| File number | ours, given at open; the path text stays in a second file beside the log (`ds:260-264`) |
| Offset, source size, destination size, intermediate size, options | the request (`ds:694-727`) |
| Destination kind: an address; pages in one run; pages apart. Source kind: file or memory | `ds:1049-1058, 998-1004` |
| Whether a name was given | `ds:714` |
| Four times: enqueued, started, finished, status visible | to be added: no time is kept per request today (`ds:353-364`) |
| Result | the request's status code |

Bounded in rows and bytes, in the manner of the validation record
(`gpu/rt/record.rs:92-96`) but with figures of its own ("Bounds"): past
them rows are counted, not written. Counts that need no history (sizes,
kinds, latencies) are running sums and are never cut; those that need
history (repeats, succession) stop at the bound, and the summary says at
which sequence number the log was cut. **Only counts leave the machine**:
a summary command prints the columns of "Measurements" and the tallies
below, and that is what is shared. Whether file names and offsets may
appear in anything shared is the owner's to say (**doc**:
`asset-policy.md`, "Allowed"); the default is no.

## What the sequence alone can predict

| Signal | Use | Standing |
|---|---|---|
| Repeats: the same file, offset, size and options asked again, in a session and across sessions | decides whether a persistent cache can hit at all | **unknown**: two logged runs over one route |
| Succession: which request follows which, kept from earlier runs | the predictor's table, as the early-pipeline list is kept from earlier runs (`gpu/pipeline/library.rs:132-136, 437-445`) | **unknown**: the same runs |
| Neighbouring offsets asked together or in order | read-ahead within a file | **unknown**: the same runs, with times |
| Batches: what is enqueued between two Submits | the unit a prediction is scored on (`ds:835-838`) | read from the code |
| Destinations | cannot be predicted: a page list is valid only at Submit (`ds:562-568`) | read from the code |
| Memory sources | cannot be anticipated: read at execution (`ds:998-1004`) | read from the code |
| Which texture a request becomes | not known from a request: the description comes at the title's resource creation | **inferred** from `gpu/resource.rs:781-783` |

**Spatial information: no reliable source was found.** A search of what
the layer sees for camera, view, frustum and position found only values
the upscaler call passes through (jitter, motion-vector scale, camera
values; no position, and only when the upscaler is configured:
`hle/fidelityfx.rs:86-97`) and gamepad state. The predictor therefore
uses the request sequence alone. Should a reliable source be shown later,
it is one more input and never a requirement.

The predictor runs first in shadow mode (`VICTORY_STREAM_PREDICT=1`,
**proposed**): it predicts and is scored against what the title then
asks, and performs no read. A prediction is scored within a horizon
counted in batches (1, 4 and 16 batches after it was issued, each
reported) and expires at the longest: without one, any later request
would match and precision would rise with the length of the session.

## Prefetch and prepare-ahead

Three things only can be done ahead: source bytes read into a pool of our
own; a decode into memory of our own; detiled bytes kept. Not possible:
writing any destination, or making a host texture before the title
describes it.

| Rule | Because |
|---|---|
| **Advisory.** Only the title's request writes title memory and a status. A result made ahead is used only when that request names the same source, offset, source size, options, destination size and intermediate size, and it is then written by the request's own path, at the request's own time | the announced write (`ds:1049-1058`; `hle/xmem.rs:514-522`); a destination valid only at Submit (`ds:562-568`) |
| **Byte-identical, with what is left unwritten.** A stored result carries its produced length; only a successful decode is stored | a zlib stream may end short and leave the rest (`ds:921-923`); a raw read writes the source's size (`ds:875-881`) |
| A failure of speculative work never reaches a status entry | statuses are the title's (`ds:637-655`) |
| **Bounded.** One pool of its own for source bytes read ahead, decodes made ahead and detiled bytes kept ("Bounds"); not counted in the title's 32 MiB, which would stall the title's requests | `ds:524-528`; `../CLAUDE.md`, the memory rule |
| **Below every title class.** Speculative work starts only when no title request waits and a worker is idle; its units are small, since a started read cannot be stopped | `ds:510-530` |
| **Cancellable.** What has not started is dropped when a title request arrives, and when its queue or file closes. A speculative read is made only on a file the title has open now, never on one known from an earlier run alone, through a handle of its own that shares reading, writing and deleting. The title's release of the file waits for the one speculative unit running on it (bounded by the unit's size) and closes that handle before it returns | a queue's close drops and waits, a file's release does not (`ds:669-692, 91-99`); the title's handle is read-only and shares reading (`ds:118-132`); a started read cannot be stopped, so a handle outliving the release would fail the title's next exclusive open, delete or rename of the file (**inferred**: `hle/files.rs:263-331` passes the title's share mode through) |
| **Reclaimed first.** The pool is given up before anything a title request needs: its way back is the file | resource-model.md, "Who reads and writes what" |
| **Checked at use.** Detiled bytes kept ahead serve a texture only if its subresource's hash then equals the hash they were made from. That hash exists today only for a subresource sharing its pages with other data; one whose changed pages hold only its own bytes is uploaded with no hash taken. So detiled bytes are kept only for subresources hashed today, or keeping them adds the hash and its time is a column. An upload prepared ahead has buffers and a lifetime of its own | `gpu/texture.rs:609-627` ("Hashing a movie frame cost 8 ms a frame"); staging is reused on the rule that uploads run only inside a waited submission (`gpu/texture.rs:94-99`) |
| **Only what a request would do.** The same bytes read, the same transforms applied; nothing deciphered | **doc**: `asset-policy.md`, "Access controls" |

Not counted anywhere today, and to be watched: buffered read-ahead fills
the system's file cache, which `physical_free_mb` counts as free (**doc**:
`memory.md`, "Four quantities"); the workers already number the logical
processors (`ds:430-442`), beside pipeline workers and texture threads
(`gpu/texture.rs:499-506`).

## The persistent cache of converted resources

It follows the caches that exist: content-addressed, published by write
then rename, refused inside a repository (`pipe/cache.rs:15-22, 37-41,
60-77`); an entry that does not decode is removed and made again
(`gpu/shader_cache.rs:228-240`). What it stores is exactly what the
decode produces: no transcode, no mip dropped, no size changed.

| | Proposed | Why |
|---|---|---|
| What | a request's decoded result; later, a detiled subresource, under a key of its own: the hash of its tiled bytes, format, dimensions, level, tile mode and the detiler's version | the two conversions that exist; a request's key has no field for a texture's layout |
| Key | converter identity and version, the BCPack tables' hash, the options (zlib bit, BC number), destination size, intermediate size, and the hash of the source bytes | a stream carries no size of its own, and the decoder's output changed once already (**doc**: `STATUS.md`, the BCPack row; `bcpack-lane-corrections.md`). The decoder has no version constant today |
| Not the key | path, offset and size | they serve stale data after a package changes (`pipe/lib.rs:8-10`) |
| Consequence | the source is read and hashed before the lookup: a hit saves the decode, not the read, and costs the hash | `pipe/cache.rs:15-22`. A locator index that saves the read is a later step, only if reads are shown to dominate |
| Bound | megabytes the owner sets: without `VICTORY_STREAM_CACHE_MB` the cache stays off. An index with sizes and last use; least recently used out first | the content cache has no size accounting, index or eviction today (`pipe/cache.rs:28-77`), and the shader cache is in the same store (`gpu/shader_cache.rs:7-8`): the index and the bound cover the stream cache's folder alone, and the shader cache gets no eviction unless a switch of its own says so |
| Invalidation | a changed key lands elsewhere; an entry whose length or hash is wrong is removed and redone; the folder may be deleted at any time | `crates/victory-core/src/config.rs:18-22` |
| Who uses it | look-ups and hits are counted for title requests only; an entry records whether a title request or speculative work wrote it | so that the hit rate is not raised by work done ahead |
| Policy | converted title data is game data: under the cache folder only, never in the repository, tests or documents; tests use generated data and assert properties | **doc**: `asset-policy.md`, "Not allowed" |

Whether a stored, larger, decoded entry can beat reading the smaller
compressed range and decoding it is **unknown**: only both paths timed on
the owner's drive and processor for the same requests can say, which is
what the service-time columns by path are for.

## Host DirectStorage, by capability

Not used today: plain file reads (`ds:124-134, 1031`), and nothing probes
storage (`gpu/caps.rs:28-70` describes the GPU only). From general
knowledge, not from this repository: the host library is a
redistributable, and its GPU decompression covers a format that neither
zlib nor BCPack streams are, so it could serve raw reads and its own
queueing only. **Proposed**, last and only if the log shows reading, not
decoding, to dominate (`ds:489-504` gives both shares in a debug line;
they become columns): a probe (library
present, factory made, the file's device), today's path as the fallback,
licence and origin recorded in `THIRD_PARTY_NOTICES.md` first. Its
behaviour under Wine, which the runtime detects (`hle/xmem.rs:38-40`), is
**unknown**.

## Measurements

Columns of the performance log, per period; today it holds
`own_loading_mb` and nothing else of streaming (`gpu/pacing.rs:184-195`).

| Quantity | Columns | Definition |
|---|---|---|
| Activity | `stream_requests`, `stream_mb` | requests completed and bytes written |
| **Streaming latency** | `stream_wait_ms_p50`, `_p95`, `_p99`; `stream_service_ms_*`; `stream_visible_ms_*`; `stream_read_ms`, `stream_decode_ms` | enqueued to started; started to finished; enqueued to status visible; worker time reading and decoding, the two shares that gate P4 |
| **Prediction accuracy** | `pred_issued`; `pred_matched_1`, `_4`, `_16`; `pred_expired`; `req_unpredicted`; `req_memory_source`; `pred_lead_ms_p50`; `pred_lead_enough` | matched: a title request within the horizon with the key of the use rule (source, offset, source size, options, destination size, intermediate size). Expired: unmatched at 16 batches. Precision is matched over issued, per horizon; coverage is file-source requests less unpredicted, over file-source requests (memory sources cannot be predicted and are counted apart). Lead is issue to the matching request's enqueue in shadow mode, result ready to that enqueue once reads are made; `pred_lead_enough` is the share of matches whose lead passes the measured p50 service time |
| **Cache hit rate** | per cache: `_lookups`, `_hits`, `_hits_speculative_entry`, `_served_mb`, `_refused`, `_evicted`, `_disk_mb`; `_hit_service_ms`, `_miss_service_ms`, `_hash_ms` | title requests only. Refused: an entry that failed its check. Service time by path is the request's started to finished, hit against miss |
| **Wasted bytes** | `pred_read_mb`; `pred_wasted_read_mb`, `pred_wasted_decode_mb`, `pred_wasted_cpu_ms` | read ahead in all; read or decoded ahead and reclaimed unused |
| **Memory overhead** | `own_stream_pool_mb`, `own_stream_tables_mb` (new owners); beside `own_loading_mb`, `physical_free_mb`, `page_faults_per_s`, `system_commit_mb` | the pool's highest bytes in the period. Buffered read-ahead in the system's file cache has no gauge of its own (`physical_free_mb` counts it as free): `pred_read_mb` is its upper figure for the period |
| **Frame-time stability** | the existing `p99_ms`, `max_ms`, `late_share`, `jitter_ms`, beside `stream_mb`, `tex_uploads`, `tex_upload_mb` in the same row | whether late frames coincide with streaming or with uploads |

## Bounds

New long-lived structures (**proposed**); each has its owner in
`account.rs` and its case in `memory.md` before it is built.

| Structure | Bound | Gauge | Lifetime |
|---|---|---|---|
| Request log | in memory a buffer of 4,096 rows (256 KiB) appended in batches; on disk 1,000,000 rows or 64 MiB a run, two runs kept | `own_stream_tables_mb`; rows written and rows cut in the summary | the run; the files until deleted |
| Set of request keys seen, for the repeat tallies | 1,000,000 64-bit hashes (8 MiB); past it repeats are no longer tallied and the summary says so | the same | the run |
| The predictor's succession table, kept across runs | 65,536 entries, 2 MiB in memory and on disk; least recently confirmed out first | the same | across runs, under the cache folder |
| The pool: source bytes read ahead, decodes made ahead, detiled bytes kept | `VICTORY_STREAM_POOL_MB`, 32 by default (the title's own in-flight bound, `ds:524-528`), 256 at most; given up whole above *pressure*'s high mark (resource-model.md) | `own_stream_pool_mb` | until used, expired or reclaimed |
| The cache's index | an entry of 48 bytes per stored file, 65,536 entries (3 MiB); the files by `VICTORY_STREAM_CACHE_MB` | `own_stream_tables_mb`; `_disk_mb` | the run; the files until evicted |

## Order, switches, tests

| Step | Switch (proposed; off by default) | Fallback |
|---|---|---|
| P0. The request log and the latency columns (step 0 of the common order) | `VICTORY_STREAM_LOG=1` | no log |
| P1. The predictor in shadow mode, scored | `VICTORY_STREAM_PREDICT=1` | no prediction |
| P2. Read-ahead of source bytes into the bounded pool | `VICTORY_STREAM_PREFETCH=1`, `VICTORY_STREAM_POOL_MB=<n>` | the request reads the file itself |
| P3a. The persistent cache, used by title requests | `VICTORY_STREAM_CACHE=1` with `VICTORY_STREAM_CACHE_MB=<n>` | the request decodes |
| P3b. Decode ahead into the pool | `VICTORY_STREAM_DECODE_AHEAD=1` (needs P2) | the request decodes |
| P4. Host DirectStorage for raw reads | `VICTORY_STREAM_HOST_DSTORAGE=1` | plain file reads |

Tested here, on any host, with the predictor, the pool and the cache index
written apart from the Windows-only module (whose own tests have not run:
**doc**, `STATUS.md`): a replay harness that feeds generated request
sequences (seeded walks over a generated file layout) and checks that, for
any prediction including wrong ones, the bytes written and the statuses
equal those of the run without prediction; that the pool never passes its
bound; that a title request waits behind at most one speculative unit;
that closing cancels, and that a release followed by an exclusive reopen
succeeds with prediction on; that the cache's bound and its eviction order
hold, a damaged entry is refused, and each field of the key changes the
key. No such harness exists today (`ds:1222-1228` builds requests by
hand).

Only the owner's machine shows: what the title asks (sizes, queues,
shares of raw, zlib and BCPack, page destinations); whether requests
repeat and neighbour; whether reading or decoding is the larger share;
whether late frames coincide with streaming; then, one switch at a time,
the columns above against a baseline row.
