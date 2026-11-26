# Progress

The working record of the compatibility effort: what has been measured, what
is blocked and on what, and what to do next. Read this first in a new
session; update it in the commit that changes the facts.

Rules that do not change between sessions:

* Only this repository is modified. The supporting repositories are
  read-only references.
* Nothing from the game enters the repository ([asset-policy.md](asset-policy.md)).
  No content-derived fingerprints of shipped data (shader or executable
  hashes) in documents either.
* No package decryption, console keys or access-control bypass, in code or
  in documents. The user supplies files they are entitled to use.
* Code is reused only under a licence compatible with GPL-3.0-or-later, with
  attribution, as a module or dependency. No licence file means no code
  reuse (documented facts only).

## Target machine

| | Target | Tested by the project so far |
|---|---|---|
| GPU | NVIDIA RTX 5070 Ti, 16 GB | AMD RX 6700 XT, 12 GB |
| CPU | Ryzen 7 5800X (8 cores, 16 threads, AVX2) | not recorded |
| RAM | 16 GB | 32 GB, commit limit about 80 GB |
| Storage | NVMe SSD | not recorded |

No run on the target machine has been recorded yet.

## Validation tiers

What a session can prove depends on where it runs.

| Tier | Needs | Command | Proves |
|---|---|---|---|
| 1 | any Linux host | `scripts/check-linux.sh tests` | host tests of every crate but `apps/victory` |
| 2 | Linux host with LLVM (clang-cl, llvm-lib, llvm-ml) | `scripts/check-linux.sh windows` | CI's clippy for `x86_64-pc-windows-msvc`, both feature sets; type-checked, not linked |
| 3 | Windows with MSVC build tools | `cargo test --workspace --locked` | everything in CI, including the Windows-only tests (loader, DXIL validator, WARP) |
| 4 | Windows, the target GPU, the user's game files | `victory translate --cache`, `victory run ... --execute` | the title itself |

A cloud session (Linux container, 4 cores, 15 GB RAM, no GPU, no game data,
no Wine, no MSVC CRT or Windows SDK) reaches tiers 1 and 2. Tiers 3 and 4
are manual tests on the owner's machine or the Windows CI job.
