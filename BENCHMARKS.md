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
| **Cold snapshot** (first commit of the whole tree) | ~2185 ms | ~329 ms | **revtool ~6.6× faster** |
| **Incremental** (re-snapshot after changing 1 of 3,000 files) | ~35 ms | ~28 ms | **revtool ~1.2× faster** |
| Storage after one commit (incompressible data) | ~72 MB | ~72 MB | tie |

## Where the speedup comes from

**Cold snapshots.** revtool's win here is *not* primarily BLAKE3. On this
hardware SHA-1/SHA-256 are hardware-accelerated (~3.5 GB/s), so the hash is not
the bottleneck. The win comes from revtool's simpler object model: it stores
objects uncompressed and does no delta encoding, while git spends most of its
commit time in zlib compression and delta/pack bookkeeping. On non-accelerated
hardware BLAKE3's throughput advantage would widen the gap further.

**Incremental snapshots.** revtool keeps a working-tree stat index
(`.rev/index`), the same idea as git's index: it remembers each file's
`(size, mtime, ctime, inode)` and the id it hashed to, then on the next
snapshot `stat()`s each file and only re-reads and re-hashes the ones whose
fingerprint changed. See [the safety discussion](#is-the-stat-cache-safe) below
— the short version is that ctime, which the OS bumps on any modification and
which cannot be forged backward, makes it safe against silently missing an edit.

## The remaining tradeoff

- **Storage vs. speed.** On *compressible* source code git would produce a
  smaller repository because it compresses objects; revtool trades disk for
  speed. On the incompressible dataset here the sizes match.

## Is the stat cache safe?

Yes, and it is designed to be provably so rather than "probably fine":

- **ctime is the safety net.** Every content change bumps the inode's ctime, and
  ctime cannot be moved backward through the normal filesystem API. So even a
  file edited in place with an identical size and a *forged-backward mtime* is
  still detected. This is covered by an adversarial test
  (`edit_with_forged_mtime_is_still_detected_via_ctime`).
- **Racy-clean guard.** A cache entry is only trusted if the file's mtime is
  strictly older (nanosecond resolution) than the moment the index was written,
  so a file touched around the time of the last snapshot is always re-verified.
- **Store verification.** A cached id is only trusted if the object is still in
  the store.
- **Escape hatch.** `revtool snap --rehash` bypasses the cache and re-reads and
  re-hashes every file.
- **Platform.** ctime is a Unix concept; on platforms without it the cache is
  disabled and every file is re-hashed — slower, but never wrong.
