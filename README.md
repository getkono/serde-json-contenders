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
| `native-counted` | `-C target-cpu=native -C target-feature=-sve,-rcpc` (aarch64 only) |

A gain that only shows up in a native build is accepted: applications are assumed to be built for the machines they run on. The decision rule is therefore evaluated at `v3` and `native`, and `portable` is reported for information only.

valgrind cannot run an aarch64 `native` build on the Neoverse N2 machines it is counted on. Its arm64 decoder (3.24.0 here, and every release up to at least 3.25.1) handles neither SVE nor the RCpc `ldapr` load, and N2 has both. Fat LTO also recompiles std with them, so every cell dies before any measured work. aarch64 `native` is therefore counted as `native-counted`: the same CPU target and tuning, with SVE (and SVE2) and RCpc (and RCpc2) switched off, and every other feature rustc reports for the CPU kept. Every SIMD path a candidate has on aarch64 is NEON, which `native-counted` keeps, so what the count leaves out is code the compiler chose to vectorise with SVE and the atomics' RCpc loads. Only the count moves: every other task still builds and measures `native` itself.

`cargo xtask doctor` checks, on each machine, that every build really runs the path it claims:

- The target features compiled into each `native` and `native-counted` binary are exactly the ones rustc reports for that CPU and that variant's `RUSTFLAGS`.
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

- **Fuzzing**: differential fuzzing (cargo-fuzz, `nightly-2026-09-25`, AddressSanitizer) covers arbitrary bytes into `Value`, arbitrary bytes into an attribute-heavy struct, and arbitrary values round-tripped through both encoders. Every target runs for 30 minutes per backend and build configuration on two libFuzzer workers, and continues past each divergence, so the whole 30 minutes is fuzzed and nothing behind the first finding is missed. Each run records how many inputs crashed, the smallest of them, its assertion, and any memory error the sanitizer reported. A divergence fails rule 2 (conformance); a sanitizer report fails rule 3 (soundness).
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
| GitHub `ubuntu-24.04-arm` (Neoverse N2) | `portable`, `native-counted` (valgrind cannot run `native`'s SVE and RCpc code) | no | no |
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

Decode-only entries are compared on the decode axes alone, and an encoder is only ever compared with other encoders. The pool excludes any entry that has failed conformance: a codec that returns wrong answers cannot decide which codecs are faster.

A backend is **recommended for Kynos** only if all four hold:

1. **Frontier.** On at least one Kynos shape, it is non-dominated among eligible entries and more than 5 % better than serde_json on decode or encode CPU or heap. This must hold at x86-64 `v3` (estimated cycles) or `native` (hardware cycles), *and* on aarch64 `native` (estimated cycles, counted at `native-counted`).
2. **Conformance.** No gating disagreement at any variant, no fuzz divergence, and no undefined behaviour under Miri.
3. **Soundness.** No advisory affecting the pinned version, no open soundness issue, and no memory error reported by the sanitizer while fuzzing.
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
| `serde_json` | serde_json | baseline | yes — the baseline | all four rules hold |
| `serde_json+float_roundtrip` | serde_json | ❌ fail | yes — unicode decode CPU 0.55× serde_json at x86_64 native (cycles). Correctly rounded float parsing, which the default parser is not (see conformance, numbers). | no route ≥10 % better in throughput or p99 beyond the A/A band |
| `sonic-rs` | sonic-rs | ❌ fail | yes — echo-post encode CPU 0.19× serde_json at x86_64 v3 (est. cycles). Lazy path lookups without a full parse (`get`, `get_many`, `LazyValue`); errors carry a byte offset as well as line and column. | grammar n_string_invalid_unicode_escape.json#ignored/owned#unique fails 4 of 4 conformance runs; fuzz decode_struct (portable); fuzz decode_struct (v3) |
| `simd-json` | simd-json | ❌ fail | yes — echo-post encode CPU 0.29× serde_json at x86_64 native (cycles). Borrowing decode of escaped strings, unescaped in place: 1,175 allocations against serde_json's 4,584 for twitter-like into borrowed types from a unique buffer. A tape API (`to_tape`) traverses without building values. | status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3) |
| `simd-json-buffers` | simd-json | ❌ fail | yes — echo-post encode CPU 0.28× serde_json at x86_64 portable (est. cycles). Reusing `Buffers` across calls roughly halves plain `from_slice`'s allocated bytes: 2.93 MB against 5.54 MB for twitter-like into borrowed types from a unique buffer. | status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3) |
| `flexon-rt` | flexon | ❌ fail | yes — sweep-256b decode CPU 0.67× serde_json at x86_64 native (cycles). `from_mut_slice` borrows escaped strings in place; JSON-pointer access. Its latest release (0.4.9) does not compile on stable Rust. | grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence |
| `flexon-rt-mut` | flexon | ❌ fail | yes — In-place parse lets escaped strings borrow: 1,170 allocations and 558 kB against `flexon-rt`'s 4,583 and 749 kB for twitter-like into borrowed types from a unique buffer. | grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence |
| `flexon-ct` | flexon | ❌ fail | yes — echo-post encode CPU 0.35× serde_json at x86_64 v3 (est. cycles). The compile-time SIMD configuration of flexon; same API as `flexon-rt`. | grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence |
| `flexon-ct-mut` | flexon | ❌ fail | yes — echo-post encode CPU 0.26× serde_json at x86_64 native (cycles). In-place parse lets escaped strings borrow: 1,170 allocations and 558 kB against `flexon-ct`'s 4,583 and 749 kB for twitter-like into borrowed types from a unique buffer. | grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence |
| `jiter` | jiter | ❌ fail | yes — floats decode CPU 0.47× serde_json at x86_64 native (cycles). Partial mode parses truncated documents (built for streamed model output); decode only. | status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3); 1 more fuzz divergence |
| `hifijson` | hifijson | ❌ fail | yes — No `unsafe`: 0 unsafe blocks, functions or impls in its 1 runtime crate. Lexes any byte iterator (`IterLexer`), so input need not be in memory; numbers kept as text. | reference point: no borrowed slice decode plus `to_vec`-style encoder |
| `struson` | struson | ❌ fail | yes — No `unsafe`: 0 unsafe blocks, functions or impls across its 5 runtime crates. A true streaming reader and writer with JSON-path seeking and positions in errors. | reference point: no borrowed slice decode plus `to_vec`-style encoder |

### The decision rule, entry by entry

| Entry point | 1 Frontier | 2 Conformance | 3 Soundness | 4 End to end |
| --- | --- | --- | --- | --- |
| `sonic-rs` | ⏳ json-small decode CPU 0.83× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ grammar n_string_invalid_unicode_escape.json#ignored/owned#unique fails 4 of 4 conformance runs; fuzz decode_struct (portable); fuzz decode_struct (v3) | ✅ no advisory at advisory-db e2111519 | ❌ no route ≥10 % better in throughput or p99 beyond the A/A band |
| `simd-json` | ⏳ json-small encode CPU 0.81× at x86_64 native (cycles); aarch64 counts not yet recorded | ❌ status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3) | ✅ no advisory at advisory-db e2111519 | ⏳ provisional pass: echo-post +13 % rps, +66 % p99 on x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor (solo); awaits a quiet-host rerun |
| `simd-json-buffers` | ⏳ json-small decode CPU 0.88× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3) | ✅ no advisory at advisory-db e2111519 | ⏳ provisional pass: echo-post -16 % rps, +65 % p99 on x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor (solo); awaits a quiet-host rerun |
| `flexon-rt` | ⏳ json-small decode CPU 0.73× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence | ✅ no advisory at advisory-db e2111519 | ❌ gains inconsistent with counted codec share: x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor json-large-post: +59 % measured, +30 % predicted |
| `flexon-rt-mut` | ⏳ json-small decode CPU 0.76× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence | ✅ no advisory at advisory-db e2111519 | ⏳ provisional pass: echo-post +8 % rps, +85 % p99 on x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor (solo); awaits a quiet-host rerun |
| `flexon-ct` | ⏳ json-small decode CPU 0.72× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence | ✅ no advisory at advisory-db e2111519 | ❌ no route ≥10 % better in throughput or p99 beyond the A/A band |
| `flexon-ct-mut` | ⏳ json-small decode CPU 0.75× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ grammar n_array_comma_after_close.json#value/owned#unique fails 4 of 4 conformance runs; Miri reports undefined behavior (in-bounds pointer arithmetic failed); fuzz decode_struct (v3); 1 more fuzz divergence | ✅ no advisory at advisory-db e2111519 | ⏳ provisional pass: json-large-post +52 % rps, +42 % p99 on x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor (solo); awaits a quiet-host rerun |
| `jiter` | ⏳ json-small decode CPU 0.90× at x86_64 v3 (est. cycles); aarch64 counts not yet recorded | ❌ status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3); 1 more fuzz divergence | ✅ no advisory at advisory-db e2111519 | ❌ no route ≥10 % better in throughput or p99 beyond the A/A band |
| `hifijson` | ❌ not on the frontier with a ≥5 % CPU or heap win on any Kynos shape at x86-64-v3 or native | ❌ status shape/valid fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3) | ✅ no advisory at advisory-db e2111519 | ❌ no route ≥10 % better in throughput or p99 beyond the A/A band |
| `struson` | ❌ not on the frontier with a ≥5 % CPU or heap win on any Kynos shape at x86-64-v3 or native | ❌ status shape/variant-wrong-type fails 4 of 4 conformance runs; fuzz decode_struct (v3); fuzz decode_value (v3); 1 more fuzz divergence | ✅ no advisory at advisory-db e2111519 | ❌ no route ≥10 % better in throughput or p99 beyond the A/A band |

### CPU per operation at the three Kynos shapes

serde_json is the absolute figure; every other column is a ratio to it (below 1 is faster). Owned decode of one shared frame, and `to_vec` encode: what a server does.

**Decode**

| Workload | Point | serde_json | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `jiter` | `hifijson` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| json-small | x86_64 v3 (est. cycles) | 4040 | 1.01× | 0.83× | 1.30× | 0.88× | 0.73× | 0.76× | 0.72× | 0.75× | 0.90× | 1.17× | 3.65× |
| json-small | x86_64 native (cycles) | 1633 | 0.65× | 0.71× | 0.83× | 0.86× | 0.68× | 0.72× | 0.65× | 0.44× | 0.62× | 0.71× | 2.55× |
| echo-post | x86_64 v3 (est. cycles) | 4471 | 1.02× | 0.60× | 1.87× | 1.19× | 0.51× | 0.63× | 0.51× | 0.62× | 0.53× | 3.01× | 14.60× |
| echo-post | x86_64 native (cycles) | 1628 | 0.60× | 0.58× | 1.22× | 0.85× | 0.52× | 0.63× | 0.48× | 0.42× | 0.59× | 1.83× | 10.14× |
| json-large | x86_64 v3 (est. cycles) | 3.88 M | 1.01× | 0.81× | 0.83× | 0.82× | 0.77× | 0.77× | 0.75× | 0.76× | 0.92× | 1.12× | 2.94× |
| json-large | x86_64 native (cycles) | 1.52 M | 0.67× | 0.56× | 0.60× | 0.71× | 0.71× | 0.61× | 0.68× | 0.40× | 0.68× | 0.73× | 2.27× |

**Encode**

| Workload | Point | serde_json | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| json-small | x86_64 v3 (est. cycles) | 1699 | 1.00× | 1.34× | 1.46× | 1.46× | 1.71× | 1.71× | 1.71× | 1.71× | 3.03× |
| json-small | x86_64 native (cycles) | 793 | 0.72× | 0.86× | 0.81× | 0.79× | 1.64× | 1.51× | 1.65× | 1.02× | 2.71× |
| echo-post | x86_64 v3 (est. cycles) | 10.6 k | 1.00× | 0.19× | 0.28× | 0.28× | 0.35× | 0.35× | 0.35× | 0.35× | 2.41× |
| echo-post | x86_64 native (cycles) | 3421 | 1.31× | 0.23× | 0.29× | 0.33× | 0.42× | 0.42× | 0.43× | 0.26× | 1.64× |
| json-large | x86_64 v3 (est. cycles) | 1.12 M | 1.00× | 0.96× | 1.34× | 1.34× | 0.97× | 0.97× | 0.97× | 0.97× | 2.39× |
| json-large | x86_64 native (cycles) | 470.6 k | 0.60× | 0.82× | 0.86× | 0.85× | 0.95× | 0.90× | 0.97× | 0.61× | 2.12× |

### Size sweep: where SIMD starts to pay

**Decode**

| Workload | Point | serde_json | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `jiter` | `hifijson` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| sweep-64b | x86_64 v3 (est. cycles) | 2879 | 1.01× | 0.82× | 1.42× | 0.93× | 0.73× | 0.77× | 0.75× | 0.78× | 0.91× | 1.15× | 4.08× |
| sweep-256b | x86_64 v3 (est. cycles) | 7244 | 1.01× | 0.75× | 1.17× | 0.85× | 0.69× | 0.69× | 0.68× | 0.68× | 0.88× | 1.26× | 4.33× |
| sweep-1kib | x86_64 v3 (est. cycles) | 43.5 k | 1.01× | 0.75× | 0.97× | 0.88× | 0.71× | 0.70× | 0.69× | 0.68× | 0.90× | 1.17× | 3.57× |
| sweep-4kib | x86_64 v3 (est. cycles) | 182.9 k | 1.00× | 0.77× | 0.96× | 0.93× | 0.72× | 0.88× | 0.70× | 0.86× | 0.90× | 1.16× | 3.42× |
| sweep-16kib | x86_64 v3 (est. cycles) | 891.6 k | 1.01× | 0.81× | 0.84× | 0.82× | 0.76× | 0.77× | 0.75× | 0.76× | 0.92× | 1.13× | 3.03× |
| sweep-64kib | x86_64 v3 (est. cycles) | 3.62 M | 1.01× | 0.81× | 0.83× | 0.82× | 0.77× | 0.77× | 0.75× | 0.76× | 0.92× | 1.13× | 3.00× |
| sweep-256kib | x86_64 v3 (est. cycles) | 14.58 M | 1.01× | 0.81× | 0.82× | 0.82× | 0.77× | 0.78× | 0.75× | 0.76× | 0.92× | 1.14× | 2.99× |
| sweep-1mib | x86_64 v3 (est. cycles) | 58.39 M | 1.01× | 0.81× | 0.89× | 0.88× | 0.77× | 0.76× | 0.75× | 0.74× | 0.92× | 1.14× | 2.98× |

**Encode**

| Workload | Point | serde_json | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| sweep-64b | x86_64 v3 (est. cycles) | 1201 | 1.00× | 1.04× | 1.09× | 1.09× | 1.75× | 1.75× | 1.75× | 1.75× | 3.30× |
| sweep-256b | x86_64 v3 (est. cycles) | 4122 | 1.00× | 0.81× | 0.85× | 0.85× | 1.17× | 1.17× | 1.17× | 1.17× | 2.63× |
| sweep-1kib | x86_64 v3 (est. cycles) | 17.6 k | 1.00× | 0.88× | 1.18× | 1.18× | 0.98× | 0.98× | 0.98× | 0.98× | 2.37× |
| sweep-4kib | x86_64 v3 (est. cycles) | 67.1 k | 1.00× | 0.90× | 1.24× | 1.24× | 0.93× | 0.93× | 0.93× | 0.93× | 2.36× |
| sweep-16kib | x86_64 v3 (est. cycles) | 280.1 k | 1.00× | 0.95× | 1.27× | 1.27× | 0.94× | 0.94× | 0.94× | 0.94× | 2.32× |
| sweep-64kib | x86_64 v3 (est. cycles) | 1.12 M | 1.00× | 0.93× | 1.26× | 1.26× | 0.94× | 0.94× | 0.94× | 0.94× | 2.34× |
| sweep-256kib | x86_64 v3 (est. cycles) | 4.47 M | 1.00× | 0.91× | 1.29× | 1.29× | 0.94× | 0.94× | 0.94× | 0.94× | 2.35× |
| sweep-1mib | x86_64 v3 (est. cycles) | 18.49 M | 1.00× | 0.90× | 1.28× | 1.28× | 0.95× | 0.95× | 0.95× | 0.95× | 2.31× |

### One variable at a time, and real-world shapes

A `—` under a nested workload is a rejection at that depth (see conformance), not a missing run.

**Decode**

| Workload | Point | serde_json | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `jiter` | `hifijson` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ints | x86_64 v3 (est. cycles) | 346.7 k | 1.14× | 0.90× | 1.19× | 1.17× | 0.86× | 0.91× | 0.82× | 0.86× | — | 2.32× | 5.04× |
| floats | x86_64 v3 (est. cycles) | 358.3 k | 1.39× | 1.03× | 0.90× | 0.89× | 0.96× | 1.01× | 0.78× | 0.83× | 1.04× | 2.18× | — |
| escapes | x86_64 v3 (est. cycles) | 517.9 k | 0.98× | 0.70× | 0.73× | 0.72× | 0.92× | 0.82× | 0.90× | 0.81× | 0.97× | 1.74× | 2.72× |
| unicode | x86_64 v3 (est. cycles) | 396.7 k | 1.01× | 0.56× | 0.79× | 0.78× | 0.55× | 0.66× | 0.52× | 0.64× | 0.98× | 1.31× | 3.47× |
| nested-100 | x86_64 v3 (est. cycles) | 99.9 k | 1.02× | 0.82× | 1.18× | 1.13× | 0.67× | 0.86× | 0.68× | 0.85× | 0.95× | — | 4.26× |
| nested-127 | x86_64 v3 (est. cycles) | 127.2 k | 1.02× | 0.83× | 1.18× | 1.14× | 0.68× | 0.87× | 0.69× | 0.86× | 0.96× | — | 4.42× |
| nested-129 | x86_64 v3 (est. cycles) | — | — | — | — | — | — | — | — | — | — | — | — |
| nested-10000 | x86_64 v3 (est. cycles) | — | — | — | — | — | — | — | — | — | — | — | — |
| wide-1000 | x86_64 v3 (est. cycles) | 1.97 M | 1.02× | 0.91× | 1.10× | 1.09× | 0.91× | 1.02× | 0.90× | 1.01× | 0.98× | 1.28× | 2.26× |
| array-small | x86_64 v3 (est. cycles) | 679.4 k | 1.06× | 0.86× | 1.04× | 1.01× | 0.75× | 0.75× | 0.72× | 0.75× | 0.96× | 1.36× | 5.11× |
| twitter-like | x86_64 v3 (est. cycles) | 25.46 M | 1.01× | 0.67× | 0.73× | 0.71× | 0.80× | 0.72× | 0.80× | 0.72× | 0.88× | — | 3.53× |
| citm-like | x86_64 v3 (est. cycles) | 15.64 M | 1.03× | 0.76× | 0.91× | 0.89× | 0.62× | 0.64× | 0.60× | 0.63× | 0.88× | — | 4.50× |
| canada-like | x86_64 v3 (est. cycles) | 53.43 M | 1.19× | 1.03× | 1.02× | 0.99× | 0.90× | 0.91× | 0.70× | 0.71× | 1.12× | — | 4.19× |

**Encode**

| Workload | Point | serde_json | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ints | x86_64 v3 (est. cycles) | 157.8 k | 1.00× | 1.05× | 1.01× | 1.01× | 1.01× | 1.01× | 1.01× | 1.01× | 3.40× |
| floats | x86_64 v3 (est. cycles) | 131.0 k | 1.00× | 1.02× | 1.77× | 1.77× | 1.00× | 1.00× | 1.00× | 1.00× | 14.10× |
| escapes | x86_64 v3 (est. cycles) | 220.9 k | 1.00× | 0.52× | 1.16× | 1.16× | 0.92× | 0.92× | 0.92× | 0.92× | 1.89× |
| unicode | x86_64 v3 (est. cycles) | 168.6 k | 1.00× | 0.30× | 0.62× | 0.62× | 0.54× | 0.54× | 0.54× | 0.54× | 1.52× |
| nested-100 | x86_64 v3 (est. cycles) | 37.1 k | 1.00× | 1.16× | 1.04× | 1.04× | 0.95× | 0.95× | 0.95× | 0.95× | 2.43× |
| nested-127 | x86_64 v3 (est. cycles) | 47.3 k | 1.00× | 1.17× | 1.05× | 1.05× | 0.95× | 0.95× | 0.95× | 0.95× | 2.40× |
| nested-129 | x86_64 v3 (est. cycles) | — | — | — | — | — | — | — | — | — | — |
| nested-10000 | x86_64 v3 (est. cycles) | — | — | — | — | — | — | — | — | — | — |
| wide-1000 | x86_64 v3 (est. cycles) | 332.3 k | 1.00× | 0.83× | 1.19× | 1.19× | 1.20× | 1.20× | 1.20× | 1.20× | 2.50× |
| array-small | x86_64 v3 (est. cycles) | 327.3 k | 1.00× | 1.31× | 1.05× | 1.05× | 0.96× | 0.96× | 0.96× | 0.96× | 2.45× |
| twitter-like | x86_64 v3 (est. cycles) | 9.55 M | 1.00× | 0.70× | 0.96× | 0.96× | 0.90× | 0.90× | 0.90× | 0.90× | 2.08× |
| citm-like | x86_64 v3 (est. cycles) | 6.17 M | 1.00× | 1.03× | 1.10× | 1.10× | 0.94× | 0.94× | 0.94× | 0.94× | 2.59× |
| canada-like | x86_64 v3 (est. cycles) | 25.15 M | 1.00× | 1.01× | 1.68× | 1.68× | 1.00× | 1.00× | 1.00× | 1.00× | 6.47× |

### Heap per operation

| Entry point | Workload | Decode allocations | Decode bytes | Decode peak | Encode allocations | Encode bytes |
| --- | --- | --- | --- | --- | --- | --- |
| `serde_json` | json-small | 5 | 123 | 671 | 1 | 128 |
| `serde_json` | echo-post | 1 | 1024 | 1024 | 1 | 2086 |
| `serde_json` | json-large | 3526 | 160.4 k | 160.4 k | 1 | 131.1 k |
| `serde_json+float_roundtrip` | json-small | 5 | 123 | 671 | 1 | 128 |
| `serde_json+float_roundtrip` | echo-post | 1 | 1024 | 1024 | 1 | 2086 |
| `serde_json+float_roundtrip` | json-large | 3526 | 160.4 k | 160.4 k | 1 | 131.1 k |
| `sonic-rs` | json-small | 5 | 123 | 671 | 1 | 256 |
| `sonic-rs` | echo-post | 1 | 1024 | 1024 | 1 | 6197 |
| `sonic-rs` | json-large | 3526 | 160.4 k | 160.4 k | 1 | 131.1 k |
| `simd-json` | json-small | 11 | 2276 | 2177 | 1 | 512 |
| `simd-json` | echo-post | 7 | 5580 | 4556 | 1 | 2086 |
| `simd-json` | json-large | 3532 | 947.1 k | 826.6 k | 1 | 131.1 k |
| `simd-json-buffers` | json-small | 7 | 838 | 838 | 1 | 512 |
| `simd-json-buffers` | echo-post | 3 | 2412 | 2412 | 1 | 2086 |
| `simd-json-buffers` | json-large | 3528 | 660.1 k | 660.1 k | 1 | 131.1 k |
| `flexon-rt` | json-small | 5 | 123 | 671 | 1 | 128 |
| `flexon-rt` | echo-post | 1 | 1024 | 1024 | 1 | 2086 |
| `flexon-rt` | json-large | 3526 | 160.4 k | 160.4 k | 1 | 131.1 k |
| `flexon-rt-mut` | json-small | 6 | 214 | 671 | 1 | 128 |
| `flexon-rt-mut` | echo-post | 2 | 2100 | 2100 | 1 | 2086 |
| `flexon-rt-mut` | json-large | 3527 | 225.9 k | 225.9 k | 1 | 131.1 k |
| `flexon-ct` | json-small | 5 | 123 | 671 | 1 | 128 |
| `flexon-ct` | echo-post | 1 | 1024 | 1024 | 1 | 2086 |
| `flexon-ct` | json-large | 3526 | 160.4 k | 160.4 k | 1 | 131.1 k |
| `flexon-ct-mut` | json-small | 6 | 214 | 671 | 1 | 128 |
| `flexon-ct-mut` | echo-post | 2 | 2100 | 2100 | 1 | 2086 |
| `flexon-ct-mut` | json-large | 3527 | 225.9 k | 225.9 k | 1 | 131.1 k |
| `jiter` | json-small | 5 | 123 | 671 | — | — |
| `jiter` | echo-post | 1 | 1024 | 1024 | — | — |
| `jiter` | json-large | 3526 | 160.4 k | 160.4 k | — | — |
| `hifijson` | json-small | 5 | 123 | 671 | — | — |
| `hifijson` | echo-post | 1 | 1024 | 1024 | — | — |
| `hifijson` | json-large | 3526 | 160.4 k | 160.4 k | — | — |
| `struson` | json-small | 14 | 675 | 671 | 4 | 155 |
| `struson` | echo-post | 8 | 1572 | 1567 | 4 | 2122 |
| `struson` | json-large | 7763 | 177.8 k | 160.9 k | 1414 | 138.7 k |

### How the body arrives, and borrowing

x86-64-v3 estimated cycles for `json-large`, relative to the same backend's owned decode of a shared frame. In-place parsers pay a copy when the frame is shared; borrowing saves the string allocations.

| Entry point | owned, shared (abs.) | owned, unique | borrowed, shared | borrowed, unique |
| --- | --- | --- | --- | --- |
| `serde_json` | 3.88 M | 1.00× | 0.92× | 0.92× |
| `serde_json+float_roundtrip` | 3.92 M | 1.00× | 0.92× | 0.92× |
| `sonic-rs` | 3.14 M | 1.00× | 0.89× | 0.89× |
| `simd-json` | 3.23 M | 0.98× | 0.92× | 0.90× |
| `simd-json-buffers` | 3.17 M | 0.98× | 0.92× | 0.89× |
| `flexon-rt` | 2.98 M | 1.00× | 0.88× | 0.88× |
| `flexon-rt-mut` | 3.00 M | 0.98× | 0.88× | 0.86× |
| `flexon-ct` | 2.91 M | 1.00× | 0.90× | 0.90× |
| `flexon-ct-mut` | 2.94 M | 0.98× | 0.90× | 0.88× |
| `jiter` | 3.58 M | 1.00× | 0.92× | 0.92× |
| `hifijson` | 4.36 M | 1.00× | 1.00× | 1.00× |
| `struson` | 11.43 M | 1.00× | 0.99× | 0.99× |

### What adopting it costs besides CPU

| Set | `.text` +full (native) | `.text` +decode-only | Clean release build | Incremental dev build | Builds on 1.85 | Declared MSRV | Crates added | `unsafe` blocks / fns / impls | Advisories |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| baseline | +47 KiB | +42 KiB | 6.2 s | 0.16 s | yes | 1.71 | 4 | 191 / 207 / 2 | none |
| baseline-float-roundtrip | +69 KiB | +64 KiB | 6.9 s | 0.22 s | yes | 1.71 | 4 | 191 / 207 / 2 | none |
| sonic-rs | +214 KiB | +180 KiB | 10.0 s | 0.20 s | yes | none | 17 | 776 / 489 / 116 | RUSTSEC-2020-0006 (patched), RUSTSEC-2022-0078 (patched), RUSTSEC-2019-0017 (patched), RUSTSEC-2023-0074 (patched) |
| simd-json | +56 KiB | +46 KiB | 7.5 s | 0.19 s | no | 1.88 | 14 | 776 / 337 / 53 | RUSTSEC-2024-0402 (patched), RUSTSEC-2019-0008 (patched) |
| flexon-rt | +74 KiB | +67 KiB | 16.0 s | 0.63 s | no | none | 2 | 165 / 165 / 0 | none |
| flexon-ct | +73 KiB | +67 KiB | 6.9 s | 0.20 s | no | none | 2 | 165 / 165 / 0 | none |
| jiter | +92 KiB | +92 KiB | 7.3 s | 0.17 s | no | 1.88 | 17 | 753 / 399 / 125 | RUSTSEC-2020-0007 (patched), RUSTSEC-2019-0017 (patched), RUSTSEC-2018-0003 (patched), RUSTSEC-2018-0018 (patched), RUSTSEC-2019-0009 (patched), RUSTSEC-2019-0012 (patched), RUSTSEC-2021-0003 (patched), RUSTSEC-2023-0074 (patched) |
| hifijson | +66 KiB | +66 KiB | 8.3 s | 0.45 s | yes | 1.56 | 1 | 0 / 0 / 0 | none |
| struson | +103 KiB | +93 KiB | 7.5 s | 0.30 s | yes | 1.85.0 | 9 | 0 / 0 / 0 | none |

Open issues touching soundness, reviewed by hand (`results/soundness.toml`); none fail rule 3 unless listed as open:

- **flexon**: no open issues matched; 0.4.9 does not compile on stable Rust (E0499), so 0.4.8 is measured
- **hifijson**: no open issues matched; the crate forbids unsafe code
- **jiter**: no open issues matched
- **serde_json**: serde-rs/json#750 PrettyFormatter::with_indent can emit non-UTF-8 to an io::Write — the issue states it is not unsound; pretty output is not used; serde-rs/json#1262, #313, #464, #753, #974 — feature requests and bugs, not memory safety
- **simd-json**: simd-lite/simd-json#446 tracking issue to re-enable Miri: the UB it lists (alloc(0) #442, aliasing #443, SSSE3 without detection #444, missing UTF-8 validation #445) was fixed before 0.18.1; Miri is still off in its CI, so this repository's Miri run is the evidence; simd-lite/simd-json#469 feature request
- **sonic-rs**: cloudwego/sonic-rs#232 get_many overflows the stack on deep nesting — a process abort (safe Rust stack overflow), in the lazy get_many API, not in serde from_slice; the depth section measures from_slice directly; cloudwego/sonic-rs#201 get_many inconsistency — behaviour, not memory safety
- **struson**: no open issues matched these terms; #21 (verify use after error) concerns logic in a crate that forbids unsafe code

### Conformance against serde_json

Disagreements per section, summed over every build variant and host; **bold** sections gate. Gating failures are listed below the table.

| Entry point | Gate | grammar | status | strings | structs | depth | numbers | attributes | value_targets | encoding | encode_diff |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `sonic-rs` | ❌ fail | **232** | 0 | 0 | 0 | **64** | **1754925** | **28** | **12** | 0 | 48 |
| `simd-json` | ❌ fail | 556 | **20** | **136** | 0 | 144 | **1754941** | **44** | 12 | **36** | 56 |
| `simd-json-buffers` | ❌ fail | 556 | **20** | **136** | 0 | 144 | **1754941** | **44** | 12 | **36** | 56 |
| `flexon-rt` | ❌ fail | **1016** | **112** | 0 | **48** | **160** | **1754933** | **124** | **676** | 40 | 72 |
| `flexon-rt-mut` | ❌ fail | **1016** | **112** | 16 | **48** | **160** | **1754933** | **124** | **676** | 40 | 72 |
| `flexon-ct` | ❌ fail | **1016** | **112** | 0 | **48** | **160** | **1754933** | **124** | **676** | 40 | 72 |
| `flexon-ct-mut` | ❌ fail | **1016** | **112** | 16 | **48** | **160** | **1754933** | **124** | **676** | 40 | 72 |
| `jiter` | ❌ fail | 328 | **12** | 0 | 0 | 144 | **1754987** | **76** | **20** | 0 | **64** |
| `hifijson` | ❌ fail | 536 | **44** | 16 | **64** | **160** | **1754966** | **236** | **20** | 0 | **128** |
| `struson` | ❌ fail | 512 | **12** | 16 | 0 | 144 | **4537502** | **44** | 12 | 76 | **128** |

- `sonic-rs` (native/sonic-rs): grammar — `n_string_invalid_unicode_escape.json#ignored/owned#unique`: n_ file accepted
- `sonic-rs` (native/sonic-rs): grammar — `n_string_invalid_unicode_escape.json#ignored/owned#shared`: n_ file accepted
- `sonic-rs` (native/sonic-rs): grammar — `n_string_invalid_unicode_escape.json#ignored/borrowed#unique`: n_ file accepted
- `sonic-rs` (native/sonic-rs): grammar — `n_string_invalid_unicode_escape.json#ignored/borrowed#shared`: n_ file accepted
- `sonic-rs` (native/sonic-rs): grammar — `y_number_minus_zero.json#value/owned#unique`: decoded value differs from serde_json
- `sonic-rs` (portable/sonic-rs+arbitrary-precision): depth — `array/ignored/2MiB/1000000`: crashed: signal 6: fatal runtime error: stack overflow, aborting
- `simd-json` (native/simd-json): status — `shape/variant-wrong-type`: status Some(422), serde_json Some(400)
- `simd-json` (native/simd-json): status — `shape/u64-out-of-range`: status Some(400), serde_json Some(422)
- `simd-json` (native/simd-json): status — `shape/negative-zero-into-u64`: status Some(200), serde_json Some(422)
- `simd-json` (native/simd-json): status — `shape/tuple-too-long`: status Some(422), serde_json Some(400)
- `simd-json` (native/simd-json): status — `shape/array-too-long`: status Some(422), serde_json Some(400)
- `simd-json-buffers` (native/simd-json): status — `shape/variant-wrong-type`: status Some(422), serde_json Some(400)
- `simd-json-buffers` (native/simd-json): status — `shape/u64-out-of-range`: status Some(400), serde_json Some(422)
- `simd-json-buffers` (native/simd-json): status — `shape/negative-zero-into-u64`: status Some(200), serde_json Some(422)
- `simd-json-buffers` (native/simd-json): status — `shape/tuple-too-long`: status Some(422), serde_json Some(400)
- `simd-json-buffers` (native/simd-json): status — `shape/array-too-long`: status Some(422), serde_json Some(400)
- `flexon-rt` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/owned#unique`: n_ file accepted
- `flexon-rt` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/owned#shared`: n_ file accepted
- `flexon-rt` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/borrowed#unique`: n_ file accepted
- `flexon-rt` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/borrowed#shared`: n_ file accepted
- `flexon-rt` (native/flexon-rt): grammar — `n_array_comma_after_close.json#ignored/owned#unique`: n_ file accepted
- `flexon-rt-mut` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/owned#unique`: n_ file accepted
- `flexon-rt-mut` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/owned#shared`: n_ file accepted
- `flexon-rt-mut` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/borrowed#unique`: n_ file accepted
- `flexon-rt-mut` (native/flexon-rt): grammar — `n_array_comma_after_close.json#value/borrowed#shared`: n_ file accepted
- `flexon-rt-mut` (native/flexon-rt): grammar — `n_array_comma_after_close.json#ignored/owned#unique`: n_ file accepted
- `flexon-ct` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/owned#unique`: n_ file accepted
- `flexon-ct` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/owned#shared`: n_ file accepted
- `flexon-ct` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/borrowed#unique`: n_ file accepted
- `flexon-ct` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/borrowed#shared`: n_ file accepted
- `flexon-ct` (native/flexon-ct): grammar — `n_array_comma_after_close.json#ignored/owned#unique`: n_ file accepted
- `flexon-ct-mut` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/owned#unique`: n_ file accepted
- `flexon-ct-mut` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/owned#shared`: n_ file accepted
- `flexon-ct-mut` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/borrowed#unique`: n_ file accepted
- `flexon-ct-mut` (native/flexon-ct): grammar — `n_array_comma_after_close.json#value/borrowed#shared`: n_ file accepted
- `flexon-ct-mut` (native/flexon-ct): grammar — `n_array_comma_after_close.json#ignored/owned#unique`: n_ file accepted
- `jiter` (native/jiter): status — `shape/variant-wrong-type`: status Some(422), serde_json Some(400)
- `jiter` (native/jiter): status — `shape/u64-out-of-range`: status Some(400), serde_json Some(422)
- `jiter` (native/jiter): status — `shape/negative-zero-into-u64`: status Some(200), serde_json Some(422)
- `jiter` (native/jiter): numbers — `negative-zero-int#f64`: both accept, values differ, and the backend's is not correctly rounded
- `jiter` (native/jiter): attributes — `flatten-map/2#owned#unique`: outcome differs from serde_json
- `jiter` (portable/jiter+arbitrary-precision): numbers — `1e400#value`: both accept, values differ, and the backend's is not correctly rounded
- `hifijson` (native/hifijson): status — `shape/valid`: status Some(400), serde_json Some(200)
- `hifijson` (native/hifijson): status — `shape/valid/null-into-option`: status Some(400), serde_json Some(200)
- `hifijson` (native/hifijson): status — `shape/valid/option-absent`: status Some(400), serde_json Some(200)
- `hifijson` (native/hifijson): status — `shape/wrong-type/string-into-u32`: status Some(400), serde_json Some(422)
- `hifijson` (native/hifijson): status — `shape/wrong-type/string-into-f64`: status Some(400), serde_json Some(422)
- `struson` (native/struson): status — `shape/variant-wrong-type`: status Some(422), serde_json Some(400)
- `struson` (native/struson): status — `shape/tuple-too-long`: status Some(422), serde_json Some(400)
- `struson` (native/struson): status — `shape/array-too-long`: status Some(422), serde_json Some(400)
- `struson` (native/struson): numbers — `random/9.14136969137253e-261`: decoded {"error":"Data: unsupported number value '9.14136969137253e-261' at path '$', line 0, column 0 (data pos 0)"}, neither the correctly rounded value nor s
- `struson` (native/struson): numbers — `random/2.5831903292054557e+102`: decoded {"error":"Data: unsupported number value '2.5831903292054557e+102' at path '$', line 0, column 0 (data pos 0)"}, neither the correctly rounded value nor

### End to end: a hyper server

**Counted, x86_64 v3** — estimated cycles per request; codec share = (backend − floor) ÷ backend, where the floor serves the same routes with no JSON.

| Route | Entry point | Cycles / request | vs serde_json | Codec share |
| --- | --- | --- | --- | --- |
| echo-post | `serde_json` | 56.1 k | 1.00× | 39 % |
| echo-post | `serde_json+float_roundtrip` | 56.3 k | 1.00× | 39 % |
| echo-post | `sonic-rs` | 45.7 k | 0.81× | 25 % |
| echo-post | `simd-json` | 53.0 k | 0.95× | 36 % |
| echo-post | `simd-json-buffers` | 49.6 k | 0.89× | 31 % |
| echo-post | `flexon-rt` | 47.7 k | 0.85× | 28 % |
| echo-post | `flexon-rt-mut` | 47.8 k | 0.85× | 29 % |
| echo-post | `flexon-ct` | 47.6 k | 0.85× | 28 % |
| echo-post | `flexon-ct-mut` | 47.6 k | 0.85× | 28 % |
| echo-post | `struson` | 132.0 k | 2.36× | 74 % |
| json-large-get | `serde_json` | 1.37 M | 1.00× | 86 % |
| json-large-get | `serde_json+float_roundtrip` | 1.33 M | 0.97× | 86 % |
| json-large-get | `sonic-rs` | 1.31 M | 0.96× | 86 % |
| json-large-get | `simd-json` | 1.75 M | 1.28× | 89 % |
| json-large-get | `simd-json-buffers` | 1.75 M | 1.28× | 89 % |
| json-large-get | `flexon-rt` | 1.32 M | 0.96× | 86 % |
| json-large-get | `flexon-rt-mut` | 1.32 M | 0.96× | 86 % |
| json-large-get | `flexon-ct` | 1.32 M | 0.96× | 86 % |
| json-large-get | `flexon-ct-mut` | 1.32 M | 0.96× | 86 % |
| json-large-get | `struson` | 2.93 M | 2.14× | 94 % |
| json-large-post | `serde_json` | 4.09 M | 1.00× | 93 % |
| json-large-post | `serde_json+float_roundtrip` | 4.12 M | 1.01× | 94 % |
| json-large-post | `sonic-rs` | 3.36 M | 0.82× | 92 % |
| json-large-post | `simd-json` | 3.35 M | 0.82× | 92 % |
| json-large-post | `simd-json-buffers` | 3.31 M | 0.81× | 92 % |
| json-large-post | `flexon-rt` | 3.15 M | 0.77× | 91 % |
| json-large-post | `flexon-rt-mut` | 3.16 M | 0.77× | 92 % |
| json-large-post | `flexon-ct` | 3.09 M | 0.75× | 91 % |
| json-large-post | `flexon-ct-mut` | 3.08 M | 0.75× | 91 % |
| json-large-post | `jiter` | 3.80 M | 0.93× | 93 % |
| json-large-post | `hifijson` | 4.53 M | 1.11× | 94 % |
| json-large-post | `struson` | 11.44 M | 2.80× | 98 % |
| json-small-get | `serde_json` | 27.6 k | 1.00× | 7 % |
| json-small-get | `serde_json+float_roundtrip` | 27.4 k | 0.99× | 6 % |
| json-small-get | `sonic-rs` | 28.8 k | 1.04× | 11 % |
| json-small-get | `simd-json` | 28.6 k | 1.04× | 11 % |
| json-small-get | `simd-json-buffers` | 28.6 k | 1.04× | 11 % |
| json-small-get | `flexon-rt` | 29.9 k | 1.08× | 14 % |
| json-small-get | `flexon-rt-mut` | 29.9 k | 1.08× | 14 % |
| json-small-get | `flexon-ct` | 29.8 k | 1.08× | 14 % |
| json-small-get | `flexon-ct-mut` | 29.8 k | 1.08× | 14 % |
| json-small-get | `struson` | 32.7 k | 1.19× | 22 % |

**Timed, x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor (solo)** — closed-loop throughput at 64 connections, and p99 open-loop at 70 % of serde_json's throughput; median of reps, relative to serde_json.

| Route | Entry point | Throughput | p99 | A/A band (rps, p99) | Counted prediction |
| --- | --- | --- | --- | --- | --- |
| echo-post | `serde_json+float_roundtrip` | -15.3 % | +39.3 % | ±22.5 %, ±27.3 % | -0.4 % |
| echo-post | `sonic-rs` | -66.7 % | +36.1 % | ±22.5 %, ±27.3 % | +22.7 % |
| echo-post | `simd-json` | +13.1 % | -65.6 % | ±22.5 %, ±27.3 % | +5.7 % |
| echo-post | `simd-json-buffers` | -15.7 % | -64.6 % | ±22.5 %, ±27.3 % | +12.9 % |
| echo-post | `flexon-rt` | -2.4 % | +15.0 % | ±22.5 %, ±27.3 % | +17.5 % |
| echo-post | `flexon-rt-mut` | +8.3 % | -84.7 % | ±22.5 %, ±27.3 % | +17.3 % |
| echo-post | `flexon-ct` | +15.3 % | +32.8 % | ±22.5 %, ±27.3 % | +17.8 % |
| echo-post | `flexon-ct-mut` | -57.9 % | +33.1 % | ±22.5 %, ±27.3 % | +17.7 % |
| echo-post | `struson` | -73.6 % | +47.3 % | ±22.5 %, ±27.3 % | -57.5 % |
| json-large-get | `serde_json+float_roundtrip` | -1.0 % | -90.2 % | ±20.4 %, ±91.7 % | +2.9 % |
| json-large-get | `sonic-rs` | -77.6 % | +237.3 % | ±20.4 %, ±91.7 % | +4.3 % |
| json-large-get | `simd-json` | -7.5 % | -37.1 % | ±20.4 %, ±91.7 % | -21.7 % |
| json-large-get | `simd-json-buffers` | -24.2 % | -29.1 % | ±20.4 %, ±91.7 % | -21.7 % |
| json-large-get | `flexon-rt` | -20.4 % | -75.4 % | ±20.4 %, ±91.7 % | +3.9 % |
| json-large-get | `flexon-rt-mut` | +8.9 % | +3.0 % | ±20.4 %, ±91.7 % | +3.9 % |
| json-large-get | `flexon-ct` | -21.5 % | -89.1 % | ±20.4 %, ±91.7 % | +3.9 % |
| json-large-get | `flexon-ct-mut` | +6.1 % | +30.6 % | ±20.4 %, ±91.7 % | +3.9 % |
| json-large-get | `struson` | -79.8 % | +268.5 % | ±20.4 %, ±91.7 % | -53.2 % |
| json-large-post | `serde_json+float_roundtrip` | -34.6 % | +507.7 % | ±21.6 %, ±10092.9 % | -0.8 % |
| json-large-post | `sonic-rs` | -41.0 % | +11116.3 % | ±21.6 %, ±10092.9 % | +21.5 % |
| json-large-post | `simd-json` | -16.2 % | +11297.9 % | ±21.6 %, ±10092.9 % | +21.9 % |
| json-large-post | `simd-json-buffers` | +16.1 % | +718.7 % | ±21.6 %, ±10092.9 % | +23.6 % |
| json-large-post | `flexon-rt` | +59.1 % | +416.6 % | ±21.6 %, ±10092.9 % | +29.9 % |
| json-large-post | `flexon-rt-mut` | +32.7 % | +53.6 % | ±21.6 %, ±10092.9 % | +29.5 % |
| json-large-post | `flexon-ct` | -73.2 % | +4813.6 % | ±21.6 %, ±10092.9 % | +32.5 % |
| json-large-post | `flexon-ct-mut` | +52.3 % | -42.3 % | ±21.6 %, ±10092.9 % | +32.8 % |
| json-large-post | `jiter` | -53.5 % | +7.0 % | ±21.6 %, ±10092.9 % | +7.6 % |
| json-large-post | `hifijson` | -83.7 % | +12976.9 % | ±21.6 %, ±10092.9 % | -9.8 % |
| json-large-post | `struson` | -70.0 % | +9313.4 % | ±21.6 %, ±10092.9 % | -64.3 % |
| json-small-get | `serde_json+float_roundtrip` | +3.2 % | -38.2 % | ±0.3 %, ±330.5 % | +0.8 % |
| json-small-get | `sonic-rs` | -25.6 % | +242.5 % | ±0.3 %, ±330.5 % | -4.2 % |
| json-small-get | `simd-json` | -54.2 % | +217.4 % | ±0.3 %, ±330.5 % | -3.7 % |
| json-small-get | `simd-json-buffers` | -2.8 % | +288.5 % | ±0.3 %, ±330.5 % | -3.7 % |
| json-small-get | `flexon-rt` | -1.8 % | +56.7 % | ±0.3 %, ±330.5 % | -7.7 % |
| json-small-get | `flexon-rt-mut` | -3.0 % | -92.7 % | ±0.3 %, ±330.5 % | -7.7 % |
| json-small-get | `flexon-ct` | +1.8 % | +177.5 % | ±0.3 %, ±330.5 % | -7.5 % |
| json-small-get | `flexon-ct-mut` | -20.4 % | +4.6 % | ±0.3 %, ±330.5 % | -7.6 % |
| json-small-get | `struson` | -7.1 % | +224.8 % | ±0.3 %, ±330.5 % | -15.6 % |

### Timed, x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor (solo)

Median ns per operation at `native`, median of rounds; ratios to serde_json. The A/A row is serde_json against itself: differences smaller than it are noise.

| Workload | Op | serde_json (ns) | A/A | `serde_json+float_roundtrip` | `sonic-rs` | `simd-json` | `simd-json-buffers` | `flexon-rt` | `flexon-rt-mut` | `flexon-ct` | `flexon-ct-mut` | `jiter` | `hifijson` | `struson` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| json-small | decode | 217 | 1.01× | 1.05× | 0.87× | 1.30× | 1.04× | 0.74× | 1.11× | 0.71× | 0.69× | 1.00× | 1.09× | 3.93× |
| json-small | encode | 103 | 1.12× | 1.11× | 1.53× | 1.29× | 1.28× | 1.82× | 1.85× | 1.70× | 1.75× | — | — | 2.58× |
| echo-post | decode | 204 | 1.00× | 0.97× | 0.69× | 2.12× | 1.43× | 0.56× | 0.68× | 0.56× | 0.68× | 0.71× | 3.00× | 16.47× |
| echo-post | encode | 450 | 1.22× | 1.25× | 0.29× | 0.36× | 0.35× | 0.47× | 0.47× | 0.46× | 0.47× | — | — | 2.37× |
| json-large | decode | 208.7 k | 0.97× | 0.96× | 0.72× | 0.79× | 0.80× | 0.69× | 0.65× | 0.61× | 0.60× | 0.95× | 1.05× | 3.28× |
| json-large | encode | 58.1 k | 0.98× | 1.00× | 1.09× | 1.40× | 1.40× | 1.02× | 1.02× | 1.04× | 1.02× | — | — | 2.28× |
| sweep-64b | decode | 178 | 0.97× | 0.99× | 0.78× | 1.33× | 1.02× | 0.61× | 0.66× | 0.67× | 0.68× | 1.15× | 1.16× | 3.84× |
| sweep-64b | encode | 63 | 1.02× | 1.03× | 1.18× | 1.05× | 1.04× | 1.95× | 1.98× | 2.06× | 2.12× | — | — | 3.33× |
| sweep-256b | decode | 403 | 1.01× | 1.02× | 0.71× | 1.28× | 0.88× | 0.88× | 0.69× | 0.60× | 0.60× | 1.12× | 1.22× | 4.47× |
| sweep-256b | encode | 207 | 1.01× | 1.05× | 1.24× | 0.96× | 0.98× | 1.12× | 1.11× | 1.15× | 1.22× | — | — | 2.40× |
| sweep-1kib | decode | 2646 | 1.00× | 0.96× | 0.82× | 0.85× | 0.76× | 0.67× | 0.68× | 0.60× | 0.59× | 0.95× | 1.05× | 3.85× |
| sweep-1kib | encode | 1277 | 0.75× | 0.82× | 0.73× | 0.90× | 0.91× | 0.79× | 0.80× | 0.79× | 0.79× | — | — | 1.63× |
| sweep-4kib | decode | 11.4 k | 0.99× | 0.97× | 0.74× | 0.87× | 0.85× | 0.70× | 0.71× | 0.66× | 0.62× | 0.98× | 1.06× | 3.69× |
| sweep-4kib | encode | 3669 | 1.00× | 1.06× | 1.04× | 2.00× | 1.28× | 0.98× | 0.99× | 0.99× | 0.99× | — | — | 2.21× |
| sweep-16kib | decode | 46.5 k | 1.01× | 0.97× | 0.76× | 0.81× | 0.80× | 0.72× | 0.70× | 0.64× | 0.63× | 0.96× | 1.14× | 3.59× |
| sweep-16kib | encode | 14.7 k | 0.98× | 1.03× | 1.02× | 1.30× | 1.30× | 0.97× | 0.96× | 0.97× | 0.98× | — | — | 2.15× |
| sweep-64kib | decode | 192.8 k | 1.00× | 0.96× | 0.75× | 0.78× | 0.80× | 0.69× | 1.07× | 0.62× | 0.62× | 0.94× | 1.07× | 3.48× |
| sweep-64kib | encode | 55.0 k | 1.03× | 1.02× | 1.12× | 1.38× | 1.41× | 0.99× | 0.99× | 1.08× | 1.02× | — | — | 2.31× |
| sweep-256kib | decode | 760.7 k | 1.01× | 0.99× | 0.75× | 0.80× | 0.79× | 0.71× | 0.69× | 0.64× | 0.66× | 0.98× | 1.10× | 3.47× |
| sweep-256kib | encode | 224.0 k | 0.99× | 1.00× | 1.40× | 1.40× | 1.46× | 0.97× | 0.98× | 1.01× | 0.99× | — | — | 2.29× |
| sweep-1mib | decode | 3.07 M | 1.09× | 1.02× | 0.93× | 0.81× | 0.79× | 0.70× | 0.69× | 0.64× | 0.62× | 0.96× | 1.08× | 3.47× |
| sweep-1mib | encode | 876.4 k | 1.01× | 1.02× | 1.02× | 1.40× | 1.41× | 0.99× | 0.99× | 1.00× | 1.00× | — | — | 2.36× |
| twitter-like | decode | 2.50 M | 0.99× | 0.63× | 0.37× | 0.38× | 0.37× | 0.45× | 0.37× | 0.44× | 0.37× | 0.60× | — | 2.31× |
| twitter-like | encode | 435.8 k | 1.02× | 1.06× | 0.80× | 1.15× | 1.15× | 0.97× | 0.95× | 1.07× | 1.02× | — | — | 3.94× |
| citm-like | decode | 796.0 k | 1.00× | 1.08× | 0.72× | 0.87× | 1.30× | 0.59× | 0.59× | 0.58× | 0.60× | 1.11× | — | 5.16× |
| citm-like | encode | 294.8 k | 1.00× | 1.01× | 1.28× | 1.21× | 1.24× | 1.00× | 1.00× | 1.02× | 1.03× | — | — | 2.65× |
| canada-like | decode | 2.73 M | 0.96× | 1.18× | 0.96× | 0.82× | 0.81× | 0.82× | 0.83× | 0.73× | 0.71× | 1.04× | — | 4.86× |
| canada-like | encode | 1.70 M | 1.02× | 1.02× | 0.99× | 1.56× | 1.59× | 1.25× | 1.02× | 0.99× | 1.00× | — | — | 5.33× |

### Limitations

- No Intel host: every host-measured x86-64 number is from AMD Ryzen 7 7800X3D 8-Core Processor.
- No macOS host: macOS timed and allocation numbers are pending a rerun on a quiet machine.
- Every timed number (timed sampling and end to end) is `solo` class: taken on a shared machine (1, 5 and 15 minute load average 2.29 2.55 3.50 as timed sampling finished), not a quieted one, so a difference inside its A/A band is noise.
- Run lengths on x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor: timed sampling 3 rounds; end to end 5 reps of 30 s per load phase; compile time 5 builds.
- Fuzzing ran 1800 s per target and configuration.

### Provenance

| Source | Host / arch | Commit | Recorded | Notes |
| --- | --- | --- | --- | --- |
| callgrind | x86_64 | `?` | 2026-09-28T08:07:42Z | valgrind-3.24.0; cpu AMD Ryzen 7 7800X3D 8-Core Processor |
| host | x86_64-linux-amd-ryzen-7-7800x3d-8-core-processor | `3df3357a` | 2026-09-28T08:17:18Z | kernel 7.2.4-200.fc44.x86_64, governor powersave, boost 1; doctor: every build runs its claimed SIMD path |


<!-- results:end -->

## Licence

MIT or Apache-2.0, at your option. The vendored JSONTestSuite files are MIT (see `references/JSONTestSuite/LICENSE`).
