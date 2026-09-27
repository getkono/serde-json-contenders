# serde-json-contenders

Every Rust web framework decodes and encodes typed JSON bodies, and nearly all of them do it with serde_json. Several libraries claim to do the same work faster, and their numbers are usually taken with the CPU tuned to the benchmark machine, parse into an untyped tree that no request handler uses, and sometimes switch on a serde_json option that slows the baseline down. This repository asks the question those numbers skip: if you replace serde_json behind a server's typed JSON body, and change nothing else, does anything get better by enough to matter? A library passes only if it is measurably better on some axis without being worse on others, returns the same values and the same errors, is sound, and still wins when the codec is one part of a whole HTTP request. The answer applies to any serde-based server, and one framework, Kynos, uses it to decide which codecs it will support.

## Methodology

### Candidates

| Crate | Version | Role | How it picks SIMD | Entry points measured |
| --- | --- | --- | --- | --- |
| serde_json | 1.0.151 | baseline | none (SWAR + `memchr`) | `from_slice`, `to_vec`, `to_writer`; also built with `float_roundtrip` |
| sonic-rs | 0.5.10 | candidate | compile time only (`avx2`+`pclmulqdq` on x86, NEON on aarch64) | `from_slice`, `to_vec`, `to_writer` |
| simd-json | 0.18.1 | candidate | run time (AVX2 → SSE4.2 → scalar; NEON) | `serde::from_slice` on a writable copy of the body (`simd-json`), and with reused `Buffers` (`simd-json-buffers`) |
| flexon | 0.4.8 | candidate | `rt`: run-time AVX2 check for container skipping only; `ct`: compile-time SSE4.2/AVX2/PCLMUL paths | `from_slice`, `from_mut_slice` (`-mut`, in place), `to_vec`, `to_writer`, each as `flexon-rt` and `flexon-ct` |
| jiter | 0.17.0 | decode-only candidate | SSE2 / NEON by architecture | `serde::from_slice` |
| hifijson | 0.5.0 | reference point, not eligible | none, no `unsafe` | `serde::exactly_one` over a slice lexer |
| struson | 0.7.2 | reference point, not eligible | none, no `unsafe` | `JsonReaderDeserializer`, `JsonWriterSerializer` |

Every version is pinned exactly. flexon's latest release, 0.4.9, does not compile on any stable Rust (E0499 in `value/lazy`), so 0.4.8, the newest release that builds, is measured instead. Not compiling on stable is itself a finding against 0.4.9.

Not measured, and why: miniserde, nanoserde, facet-json and merde_json do not implement serde's traits, so they cannot sit behind a serde body type. serde_json_lenient and the json5 crates accept a looser grammar than JSON. serde-json-core is `no_std` and does not escape strings. json-rust and serde_jsonrc are unmaintained. asmjson describes itself as experimental.

hifijson and struson are measured and tested like the rest, and so appear in the "deserves to exist" column. Neither can be recommended as a server codec, because neither offers a borrowing decode from a slice together with a `to_vec`-style encoder.

### What a backend must do

The contract is what a server's typed JSON body does (`crates/codecs`):

- Decode `T` from the `Bytes` the server received, into an owned `T` or a `T` that borrows from the body.
- Encode `T` into a fresh `Vec` (what a server calls today), or into a buffer that is reused and already has capacity.
- Classify each failure as the client's syntax (400) or as well-formed JSON of the wrong shape (422), using serde_json's `Data` category as the reference.

Any copy a backend needs in order to meet the contract happens inside the adapter, so it is counted. For example, simd-json parses in place, so a body that shares its allocation has to be copied first.

### Builds

Each backend crate is built on its own, as its own cargo invocation with its own target directory, and always beside serde_json. Cargo's feature unification therefore cannot change another backend's features. For every build, the serde_json features that cargo resolves are compared with the ones the build is supposed to have, and a build that differs is refused.

Every build uses the same profile: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "unwind"`, and line tables only, which change no code generation. Every build uses the same compiler, Rust 1.97.1. Each build is repeated at three CPU targets, and the baseline is built at every one of them:

| Variant | `RUSTFLAGS` |
| --- | --- |
| `portable` | `-C target-cpu=x86-64` (aarch64: `generic`) |
| `v3` | `-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq` (x86 only) |
| `native` | `-C target-cpu=native` |

A gain that only shows up in a native build is accepted: applications are assumed to be built for the machines they run on. The decision rule is therefore evaluated at `v3` and `native`, and `portable` is reported for information only.

`cargo xtask doctor` checks, on each machine, that every build really runs the path it claims:

- The target features compiled into each `native` binary are exactly the ones rustc reports for that CPU.
- sonic-rs took its compile-time fast path.
- simd-json and flexon selected their best path at run time.

A run on a machine where any of these checks fails is refused.

### Axes

| # | Axis | Instrument | Where |
| --- | --- | --- | --- |
| 1 | Decode CPU | callgrind instructions and estimated cycles (`Ir + 5·L1 misses + 35·LL misses`, with a fixed cache geometry of 32 KiB 8-way L1 and 8 MiB 16-way LL, so the figure does not depend on the host); hardware `cycles:u` and `instructions:u` through `perf_event_open`; wall-clock time | callgrind in a container pinned by digest (valgrind 3.24.0, Debian snapshot 2026-09-25); counters and time on the host |
| 2 | Encode CPU | same | same |
| 3 | Heap | a counting global allocator: allocations, reallocations, bytes, peak live bytes per operation | host |
| 4 | Binary size | `.text` of a program that decodes and re-encodes the two body types a handler would declare, minus the `.text` of the same program without JSON; a decode-only program as well | host |
| 5 | Compile time | clean release build, clean dev build, and incremental dev rebuild after touching the program; median of 5 runs; no compiler cache | host |
| 6 | MSRV | declared `rust-version`, and whether the adapter builds with Rust 1.85 | host |
| 7 | `unsafe` and soundness | `unsafe` blocks, functions, impls and traits (parsed with `syn`) in every crate the backend adds to the build; RustSec advisories at advisory-db `e2111519`; open soundness issues, reviewed by hand (`results/soundness.toml`) | host |
| 8 | Conformance | the differential suite below | gate, not trade-off |

Each counted measurement follows the same protocol:

1. The operation runs once and its result is checked. A wrong result is reported and never measured.
2. A warm-up call pays one-time costs: CPU detection, lazy statics, first-touch page faults.
3. The operation is repeated inside a function that callgrind is told to count. That is `max(1, min(1000, 4 MiB ÷ size))` iterations, each on a fresh input, and the result is dropped inside the counted region, as a server drops it.

### Workloads

Every workload is a typed Rust struct, never an untyped tree. Each one comes in an owned form (`String` fields) and a borrowed form (`#[serde(borrow)] Cow<'de, str>`). All inputs are generated from a fixed splitmix64 stream and pinned by SHA-256 (`crates/payloads/SHA256SUMS`).

| Group | Workloads | Varies |
| --- | --- | --- |
| Kynos shapes (decision rule) | `json-small` (91 B `Item`), `echo-post` (1 KiB `Echo`, decoded and re-encoded), `json-large` (64 KiB `Catalog`, 705 items) | byte-identical to kynos-bench's golden bodies, asserted by digest |
| Size sweep | `Vec<Item>` at exactly 64 B, 256 B, 1 KiB, 4 KiB, 16 KiB, 64 KiB, 256 KiB, 1 MiB | where SIMD setup starts paying for itself |
| Families (about 16 KiB each) | `ints` (every digit count, signed and unsigned), `floats` (uniform over finite bit patterns), `escapes` (about 25 % escaped bytes), `unicode` (CJK, Hangul, Cyrillic, Greek, emoji), `nested-{100,127,129,10000}` (object depth either side of serde_json's limit of 128), `wide-1000` (a 1 000-key map), `array-small` (a long `Vec<Point>`) | one variable at a time |
| Real-world schemas | `twitter-like` (800 KiB, non-ASCII written as `\u` escapes), `citm-like` (480 KiB, integer- and key-heavy), `canada-like` (2.1 MiB of full-precision floats) | the schemas of the three documents most JSON benchmarks use; the originals carry no licence that permits redistribution, so they are regenerated, not copied |

### How the body arrives and how the response leaves

Decode input is `Bytes` in two forms:

- **`shared`**: one frame split off a larger buffer, as hyper hands out a single-frame body. It is not unique, so an in-place parser has to copy it.
- **`unique`**: a fresh buffer, as a multi-frame body is after collecting.

Encoding is measured two ways: `to_vec` into a new `Vec`, and `to_writer` into a reused `Vec` that already has capacity.

### Conformance

The differential suite is `crates/conformance`. It compares every backend with serde_json in the same build, and runs at every variant, because SIMD paths differ between variants. It also runs once with serde_json's `arbitrary_precision` unified in.

| Section | What | Gates? |
| --- | --- | --- |
| grammar | JSONTestSuite `test_parsing` (95 `y_`, 188 `n_`, 35 `i_`, vendored with licence and digests), decoded into `Value` and into `IgnoredAny` (which catches lazy skipping that never validates), for both arrivals | `y_`/`n_` yes; `i_` recorded |
| status | 400/422 for every `n_` case, wrong-shape inputs, and every truncation of the three Kynos bodies | yes |
| strings | lone and paired surrogates, invalid UTF-8, into `String`, `&str` and `Cow` | yes |
| structs | duplicate fields, `deny_unknown_fields` | yes |
| depth | 100 up to 1 000 000 levels, run in a subprocess on 2 MiB (tokio's worker stack) and 8 MiB stacks | a crash gates; the limit itself is recorded |
| numbers | edge values; 1 000 000 random `f64` bit patterns. A decode passes if it is correctly rounded (the truth is `str::parse`) or bit-identical to serde_json's own decode in the same build. Encoders must round-trip exactly. | yes |
| attributes | `flatten`, `untagged`, internally, adjacently and externally tagged enums, `alias`, `rename_all`, `default`, `skip`, `Option` | yes |
| value_targets | decoding into `serde_json::Value` and `Map` | yes |
| encoding | control characters, `/`, `<`, U+2028, U+2029, NaN, ±∞ | invalid or different output yes; byte differences, and refusing to encode NaN or ±∞ (serde_json silently writes `null`), recorded |
| encode_diff | every workload, byte for byte against serde_json; differing bytes are compared exactly for meaning | semantic differences yes |

Beyond the differential suite:

- **Fuzzing**: differential fuzzing (cargo-fuzz, `nightly-2026-09-25`, 30 minutes per target per backend) covers arbitrary bytes into `Value`, arbitrary bytes into an attribute-heavy struct, and arbitrary values round-tripped through both encoders.
- **Miri**: runs a subset of the suite wherever the backend's intrinsics allow it.

A backend that disagrees silently with serde_json on anything that gates fails. Two rules decide what counts as a disagreement:

- **Untyped values are judged against the input.** A decoded `Value` passes if it equals serde_json's, or if it holds exactly what the input text says, with numbers compared as values. Floats count as equal when both are correctly rounded, and `-0` counts as equal to the integer `0`. Without this rule, every backend that rounds correctly would fail wherever serde_json's default parser does not. Differences allowed by this rule are still listed.
- **Typed targets are strict.** An `f64` field decoded from `-0.0` must keep its sign.

Each adapter maps its crate's errors to 400/422 as faithfully as that crate's error type allows. simd-json's own `is_data()` counts a malformed literal (`[fals]`) and its depth limit as shape errors, so its adapter corrects both.

### End to end

`crates/e2e` is a hyper 1.x server that reads each body the way a hyper-based framework does (`collect().to_bytes()`) and uses one backend per build. Its routes are `POST /echo`, `GET /json/large`, `POST /json/large` (whose 64 KiB body arrives in several frames) and `GET /json/small`. A *floor* build serves the same routes with no JSON: it returns pre-encoded bytes and discards request bodies.

- **Counted**: callgrind per request over an in-memory connection. The codec's share of a request is (backend − floor) ÷ backend. That share bounds what swapping the codec can gain end to end. A timed gain larger than the count predicts is treated as a measurement error, not a result.
- **Timed**: loopback, with the server on cores 0–3 and oha 1.16.0 on cores 8–15. Closed-loop throughput is measured at 64 connections, then open-loop p99 at 70 % of serde_json's throughput (with coordinated-omission correction). Each measurement is 30 s, the backends run 5 times in shuffled order, and serde_json runs twice as an A/A control.

### Timed measurements and trust

Wall-clock numbers corroborate the counts; they do not decide anything on their own. Each timed cell is one process:

- **Pinning**: on Linux the process is pinned to one core.
- **Rejecting disturbed samples**: a 20 ms sample is discarded and repeated if more than 2 % of it went to other processes. On Linux that is the pinned core's run time (from `/proc/schedstat`) minus this thread's CPU time. On macOS, which has no core pinning, it is machine-wide busy ticks, with 100 ms windows.
- **Sampling**: 100 samples after a 3 s warm-up.
- **Ordering**: cells run in an order shuffled per round, over 3 rounds, and serde_json runs twice per round as the A/A control.

This replaces criterion. Rejecting a disturbed sample and interleaving processes across rounds are the two things the protocol needs, and neither can be done inside one criterion run.

Every timed number carries a trust class:

- **`solo`**: a shared machine with checked windows.
- **`canonical`**: a quiet, dedicated machine, stated with `--trust canonical`.

A pass under rule 4 that rests on `solo` numbers is reported as provisional.

### Hosts

| Host | Counted (callgrind) | Hardware counters | Timed |
| --- | --- | --- | --- |
| AMD Ryzen 7 7800X3D (Zen 4), Linux | `portable`, `v3` in the container (valgrind cannot run AVX-512, so not `native`) | all variants | `solo` |
| Apple Silicon, macOS | not available (no valgrind) | not available (needs root) | `solo`, when recorded |
| GitHub `ubuntu-24.04-arm` (Neoverse) | `portable`, `native` | no | no |
| GitHub `ubuntu-24.04` | reproducibility check only: counts must match the committed ones within 0.5 % | no | no |

There is no Intel machine, so Intel wall-clock and hardware-counter numbers are missing. Timed numbers are superseded whenever `mise run all` is rerun on a machine that has been quieted.

### Decision rule

*Dominance* is ε-dominance with ε = 5 %. One entry dominates another when it is no more than 5 % worse on every axis and more than 5 % better on at least one. The axes are:

- decode cycles
- encode cycles
- decode heap bytes
- encode heap bytes
- `.text` added
- clean release compile time

Decode-only entries are compared on the decode axes alone.

A backend is **recommended for Kynos** only if all four hold:

1. **Frontier.** On at least one Kynos shape, it is non-dominated among eligible entries and more than 5 % better than serde_json on decode or encode CPU or heap. This must hold at x86-64 `v3` (estimated cycles) or `native` (hardware cycles), *and* on aarch64 `native`.
2. **Conformance.** No gating disagreement at any variant, no fuzz divergence, and no undefined behaviour under Miri.
3. **Soundness.** No advisory affecting the pinned version, and no open soundness issue.
4. **End to end.** At least 10 % better throughput or p99 on `echo-post` or `json-large`, outside the A/A band, and consistent with the counted codec share.

Separately from Kynos, a library **deserves to exist** if either of these holds:

- It is on the frontier, with a win of more than 5 %, for *any* workload, variant or architecture.
- It offers a capability serde_json lacks. The capability is stated with its evidence.

### Reproduce

```sh
mise install              # toolchain, nextest, convco, oha
mise run all              # every measurement this machine can take, then the report
cargo xtask report        # regenerate the results below from results/
```

Individual tasks are listed by `cargo xtask`. Every result file under `results/` records its commit, host, compiler and container image.

## Results

<!-- results:begin (generated by `cargo xtask report`; do not edit) -->

### Verdict

| Entry point | Crate | Add to Kynos? | Deserves to exist? | Why |
| --- | --- | --- | --- | --- |
| `serde_json` | serde_json | baseline | yes — the baseline | footprint not yet recorded |
| `serde_json+float_roundtrip` | serde_json | ⏳ pending | yes — Correctly rounded float parsing, which the default parser is not (see conformance, numbers). | footprint not yet recorded |
| `sonic-rs` | sonic-rs | ⏳ pending | yes — Lazy path lookups without a full parse (`get`, `get_many`, `LazyValue`); errors carry a byte offset as well as line and column. | x86-64 counts not yet recorded |
| `simd-json` | simd-json | ⏳ pending | yes — Borrowing decode of escaped strings, unescaped in place; a tape API (`to_tape`) for traversal without building values. | x86-64 counts not yet recorded |
| `simd-json-buffers` | simd-json | ⏳ pending | yes — Reusing `Buffers` across calls removes the per-call scratch allocations plain `from_slice` makes. | x86-64 counts not yet recorded |
| `flexon-rt` | flexon | ⏳ pending | yes — `from_mut_slice` borrows escaped strings in place; JSON-pointer access. Its latest release (0.4.9) does not compile on stable Rust. | x86-64 counts not yet recorded |
| `flexon-rt-mut` | flexon | ⏳ pending | yes — In-place parse lets escaped strings borrow. | x86-64 counts not yet recorded |
| `flexon-ct` | flexon | ⏳ pending | yes — The compile-time SIMD configuration of flexon; same API as `flexon-rt`. | x86-64 counts not yet recorded |
| `flexon-ct-mut` | flexon | ⏳ pending | yes — In-place parse lets escaped strings borrow. | x86-64 counts not yet recorded |
| `jiter` | jiter | ⏳ pending | yes — Partial mode parses truncated documents (built for streamed model output); decode only. | x86-64 counts not yet recorded |
| `hifijson` | hifijson | ❌ fail | yes — No `unsafe`; lexes any byte iterator (`IterLexer`), so input need not be in memory; numbers kept as text. | reference point: no borrowed slice decode plus `to_vec`-style encoder |
| `struson` | struson | ❌ fail | yes — No `unsafe`; a true streaming reader and writer with JSON-path seeking and positions in errors. | reference point: no borrowed slice decode plus `to_vec`-style encoder |

### The decision rule, entry by entry

| Entry point | 1 Frontier | 2 Conformance | 3 Soundness | 4 End to end |
| --- | --- | --- | --- | --- |
| `sonic-rs` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `simd-json` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `simd-json-buffers` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `flexon-rt` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `flexon-rt-mut` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `flexon-ct` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `flexon-ct-mut` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `jiter` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `hifijson` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |
| `struson` | ⏳ x86-64 counts not yet recorded | ⏳ conformance not yet run | ⏳ footprint not yet recorded | ⏳ end-to-end timing not yet recorded |

### CPU per operation at the three Kynos shapes

serde_json is the absolute figure; every other column is a ratio to it (below 1 is faster). Owned decode of one shared frame, and `to_vec` encode: what a server does.

### Size sweep: where SIMD starts to pay

### One variable at a time, and real-world shapes

A `—` under a nested workload is a rejection at that depth (see conformance), not a missing run.

### Provenance

| Source | Host / arch | Commit | Recorded | Notes |
| --- | --- | --- | --- | --- |


<!-- results:end -->

## Licence

MIT or Apache-2.0, at your option. The vendored JSONTestSuite files are MIT (see `references/JSONTestSuite/LICENSE`).
