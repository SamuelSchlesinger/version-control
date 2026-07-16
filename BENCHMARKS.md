# Benchmarks

These are measured comparisons against git, not marketing numbers. The harness
and dataset are described below so the results are reproducible, and the honest
tradeoffs are called out.

## Setup

- **Dataset:** ~3,000 files across 60 directories plus 5×10 MB blobs, ~72 MB
  total, random (incompressible) content so neither tool benefits from
  deduplication or compression of the payload.
- **Method:** each operation is run in a fresh repository 7 times; the median
  wall-clock time is reported. git runs with `commit.gpgsign=false` and
  `--no-verify`.
- **Hardware:** Apple Silicon (arm64), which has hardware-accelerated SHA — an
  important caveat noted under "Where the speedup comes from".
- **Build:** `cargo build --release`.

## Results

| Operation | git | revtool | Winner |
|-----------|-----|---------|--------|
| **Cold snapshot** (first commit of the whole tree) | ~2185 ms | ~327 ms | **revtool ~6.7× faster** |
| **Incremental** (re-snapshot after changing 1 of 3,000 files) | ~37 ms | ~135 ms | **git ~3.6× faster** |
| Storage after one commit (incompressible data) | ~72 MB | ~72 MB | tie |

## Where the speedup comes from

revtool's cold-snapshot win is **not** primarily BLAKE3. On this hardware SHA-1
and SHA-256 are hardware-accelerated (~3.5 GB/s), so the hash is not the
bottleneck. The win comes from revtool's simpler object model: it stores objects
uncompressed and does no delta encoding, while git spends most of its commit time
in zlib compression and delta/pack bookkeeping. On non-accelerated hardware
BLAKE3's throughput advantage would widen the gap further.

The flip side is the honest tradeoff:

- **Storage vs. speed.** On *compressible* source code git would produce a
  smaller repository because it compresses objects; revtool trades disk for
  speed. On the incompressible dataset here the sizes match.
- **Incremental commits.** git maintains an index (staging area) and only
  re-hashes files whose stat data changed, so a one-file commit is very fast.
  revtool re-walks and re-hashes the whole working tree on every snapshot, so it
  is slower for small, frequent commits on a large tree. This is a deliberate
  design choice: the alternative (a stat-based hash cache) trades a small but
  real risk of missing an in-place edit whose size and mtime are unchanged —
  precisely the class of silent-data-loss bug this project prioritizes avoiding.

## Takeaway

revtool is substantially faster for **whole-tree** operations (snapshot,
checkout, full-tree diff) because of its simpler model, at the cost of
incremental-commit speed on large trees and (on compressible data) storage size.
Pick the tradeoff that fits the workload.
