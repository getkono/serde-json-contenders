# serde-json-contenders

Every Rust web framework decodes and encodes typed JSON bodies, and nearly all of them do it with serde_json. Several libraries claim to do the same work faster, and their numbers are usually taken with the CPU tuned to the benchmark machine, parse into an untyped tree that no request handler uses, and sometimes switch on a serde_json option that slows the baseline down. This repository asks the question those numbers skip: if you replace serde_json behind a server's typed JSON body, and change nothing else, does anything get better by enough to matter? A library passes only if it is measurably better on some axis without being worse on others, returns the same values and the same errors, is sound, and still wins when the codec is one part of a whole HTTP request. The answer applies to any serde-based server, and one framework, Kynos, uses it to decide which codecs it will support.

This README holds the method and the commentary. Every measured number, table and figure is generated from `results/` into [`report/`](report/README.md) by `cargo xtask report`; nothing in this file is a measurement that the report does not show.

## Scope: x86-64 Linux only

This benchmark measures x86-64 Linux, and only that. Its instruments are Linux's: `perf_event_open` for hardware cycles, `/proc/schedstat` and `sched_setaffinity` for pinning and for rejecting disturbed samples, `taskset` for the end-to-end server, and valgrind's callgrind for deterministic counts. Its builds are x86-64 builds: `x86-64`, `x86-64-v3`, and the host's own CPU.

arm64 is excluded on purpose. Hardware cycles decide the rule (see [Axes](#axes)), and there is no arm64 server to take them on. An instruction count alone cannot stand in for them: instructions are not equally expensive, so a count can rank two codecs in the wrong order. The earlier arm64 counts, which valgrind could only take with SVE and RCpc switched off, have been dropped. macOS goes with arm64: its hosts are Apple Silicon, and it has none of the instruments above.

## Methodology

### Candidates

| Crate | Version | Role | How it picks SIMD on x86-64 | Entry points measured |
| --- | --- | --- | --- | --- |
| serde_json | 1.0.151 | baseline | none (SWAR + `memchr`) | `from_slice`, `to_vec`, `to_writer`; also built with `float_roundtrip` |
| sonic-rs | 0.5.10 | candidate | compile time only (`avx2`+`pclmulqdq`) | `from_slice`, `to_vec`, `to_writer` |
| simd-json | 0.18.1 | candidate | run time (AVX2 → SSE4.2 → scalar) | `serde::from_slice` on a writable copy of the body (`simd-json`), and with reused `Buffers` (`simd-json-buffers`) |
| flexon | 0.4.8 | candidate | `rt`: run-time AVX2 check for container skipping only; `ct`: compile-time SSE4.2/AVX2/PCLMUL paths | `from_slice`, `from_mut_slice` (`-mut`, in place), `to_vec`, `to_writer`, each as `flexon-rt` and `flexon-ct` |
| jiter | 0.17.0 | decode-only candidate | SSE2 | `serde::from_slice` |
| hifijson | 0.5.0 | reference point, not eligible | none, no `unsafe` | `serde::exactly_one` over a slice lexer |
| struson | 0.7.2 | reference point, not eligible | none, no `unsafe` | `JsonReaderDeserializer`, `JsonWriterSerializer` |

Every version is pinned exactly. flexon's latest release, 0.4.9, does not compile with Rust 1.97.1, the compiler every build here uses (E0499, a double mutable borrow in `value/lazy`; checked with `cargo check` on 2026-10-15), so 0.4.8, the newest release that builds, is measured instead.

hifijson and struson are measured and tested like the rest, and so appear in the "deserves to exist" column. Neither can be recommended as a server codec, because neither offers a borrowing decode from a slice together with a `to_vec`-style encoder.

### Not measured, and why

None of these can sit behind a serde body type, so measuring one means writing a second set of body types with its own derive. That is a different change from "replace serde_json and change nothing else". Each also has its own reason, checked on 2026-10-15:

- **facet-json** (facet 0.46.1, 0.50.0-rc.7) is the only one built for typed bodies at this scope. It has borrowing decode, `flatten`, all three enum taggings, `rename_all`, `deny_unknown_fields`, and errors that separate syntax from type mismatches. But it is reflection-based by design: "facet's goal is not speed" ([facet.rs/guide/why](https://facet.rs/guide/why/)). The Cranelift JIT fast path that beat serde_json was removed in facet-json 0.46.0 on 2026-04-15 ([changelog](https://github.com/facet-rs/facet-format/blob/main/CHANGELOG.md)), and its experimental successor was deleted in August 2026 ([facet#2649](https://github.com/facet-rs/facet/pull/2649)). The path that ships decoded at a geometric mean of about 8× serde_json's instructions, and encoded at about 2×, across facet's own 30 JSON benchmarks in the last published run ([perf.facet.rs](https://perf.facet.rs/index.json), 2026-01-25). Nothing published since the removal shows otherwise. It would be measured to confirm it is slower, at the cost of a parallel `#[derive(Facet)]` copy of every fixture.
- **merde / merde_json** was archived on 2025-06-03 and is deprecated by its author in favour of facet ([bearcove/merde](https://github.com/bearcove/merde)).
- **miniserde** trades speed for small code and no recursion, and its author expects it to be up to about 2× slower than serde_json ([dtolnay/miniserde](https://github.com/dtolnay/miniserde)). Its decode is owned-only, from `&str`. It supports unit enums only, and `rename` is its only attribute. Its error type carries no information, so it cannot tell a client's syntax error (400) from the wrong shape (422).
- **nanoserde** decodes from `&str` a character at a time, with no borrowing, no `flatten`, no enum tagging and no `rename_all` ([not-fl3/nanoserde](https://github.com/not-fl3/nanoserde)). Its last release was 0.2.1 on 2025-03-22.
- serde_json_lenient and the json5 crates accept a looser grammar than JSON. serde-json-core is `no_std` and does not escape strings. json-rust and serde_jsonrc are unmaintained. asmjson describes itself as experimental.

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
| `portable` | `-C target-cpu=x86-64` |
| `v3` | `-C target-cpu=x86-64-v3 -C target-feature=+pclmulqdq` |
| `native` | `-C target-cpu=native` |

A gain that only shows up in a native build is accepted: applications are assumed to be built for the machines they run on. The decision rule is evaluated at `native`. `v3` corroborates it, because valgrind cannot run the AVX-512 code a `native` build emits on the measured host. `portable` is counted for information only: it appears in no table, and its counts are in `report/data/callgrind.csv`.

`cargo xtask doctor` checks, on each machine, that every build really runs the path it claims:

- The SIMD-relevant target features compiled into each binary (SSE2 to AVX-512) are exactly the ones rustc reports for that CPU and that variant's `RUSTFLAGS`.
- simd-json reports the path it selected, which must be its best for the CPU.
- sonic-rs and flexon select theirs with a compile-time `cfg` or a CPUID check; `doctor` evaluates the same condition inside the built binary.

Every measuring task (`count`, `alloc`, `hw`, `time`, `e2e-count`, `e2e-time`) refuses to start unless `doctor` passed on this host for the same code, and the report refuses a doctor result with failures.

### Axes

| # | Axis | Instrument | Where |
| --- | --- | --- | --- |
| 1 | Decode CPU | **hardware `cycles:u`** at `native`, through `perf_event_open`: this decides. callgrind estimated cycles (`Ir + 5·L1 misses + 35·LL misses`, with a fixed cache geometry of 32 KiB 8-way L1 and 8 MiB 16-way LL, so the figure does not depend on the host) and instructions at `v3` corroborate it and reproduce exactly. Wall-clock time is reported for information. | cycles and time on the host; callgrind in a container pinned by digest (valgrind 3.24.0, Debian snapshot 2026-09-25) |
| 2 | Encode CPU | same | same |
| 3 | Heap | a counting global allocator, built only into the binary that counts allocations (its atomic counters would add to every other figure): allocations, reallocations, bytes, peak live bytes per operation, at `native` | host |
| 4 | Binary size | `.text` of a program that decodes and re-encodes the two body types a handler would declare, minus the `.text` of the same program without JSON; a decode-only program as well | host |
| 5 | Compile time | clean release build, clean dev build, and incremental dev rebuild after touching the program; median of 5 runs; no compiler cache | host |
| 6 | MSRV | declared `rust-version`, and whether the adapter type-checks (`cargo check`) with Rust 1.85 | host |
| 7 | `unsafe` and soundness | `unsafe` blocks, functions (trait method declarations included), impls and traits, parsed with `syn` from every Rust file of every crate the backend adds to the build (its build script included; its tests, benches and examples not), plus every `unsafe` token inside a macro body or invocation, counted separately as well; RustSec advisories at a pinned advisory-db commit; open soundness issues, reviewed by hand (`results/soundness.toml`) | host |
| 8 | Conformance | the differential suite below | gate, not trade-off |

Cycles decide because instructions are not equally expensive: a vector instruction, a mispredicted branch and a cache miss each cost a different number of cycles, and an instruction count weighs them all the same. Cycles are noisy, so they decide only through a protocol that measures their noise. callgrind's counts have no noise at all, so a cycles win that the count contradicts is not accepted.

Each per-operation measurement follows the same protocol:

1. The operation runs once and its result is checked. A wrong result is reported and never measured.
2. A warm-up call pays one-time costs: CPU detection, lazy statics, first-touch page faults.
3. The operation is repeated inside a function that is the only thing counted. That is `max(1, min(1000, 4 MiB ÷ size))` iterations, and the result is dropped inside the counted region, as a server drops it. A `unique` arrival gets a fresh copy of the input each iteration. A `shared` arrival clones one `Bytes` handle each iteration, so the input stays in cache, as a frame a server has just read does.

Hardware cycles add the timed protocol below. Each cell is its own process, pinned to one core (a process that cannot pin is an error, not a figure), and every repetition (101 per process, the median kept) during which another task ran on that core for more than 2 % of the time is discarded and repeated. The core's SMT sibling shares its caches and pipeline, so it must be idle too: its busy share is read from `/proc/stat` over 100 ms before the process, over the process, and over 100 ms after, and a cell is rerun, up to 30 times, while it exceeds 10 %. Cells run one at a time, in 3 rounds in shuffled order, with serde_json twice per round as the A/A control. The figure is the median over rounds, and a timed task always runs with the baseline in the same session, replacing its result file rather than merging into an earlier session's (a run narrowed to some sets or workloads leaves the file incomplete, which the report refuses).

### Workloads

Every workload is a typed Rust struct, never an untyped tree. Each has an owned form (`String` fields) and a borrowed form (`#[serde(borrow)] Cow<'de, str>`) wherever it has strings; a workload of numbers only has the one form, measured twice. All inputs are generated from a fixed splitmix64 stream and pinned by SHA-256 (`crates/payloads/SHA256SUMS`).

| Group | Workloads | Varies |
| --- | --- | --- |
| Kynos shapes (decision rule) | `json-small` (91 B `Item`), `echo-post` (1 076 B `Echo`, a 1 KiB ASCII payload), `json-large` (64 KiB `Catalog`, 705 items) | byte-identical to kynos-bench's golden bodies, asserted by digest |
| Size sweep | `Vec<Item>` at exactly 64 B, 256 B, 1 KiB, 4 KiB, 16 KiB, 64 KiB, 256 KiB, 1 MiB | where SIMD setup starts paying for itself |
| Families (about 16 KiB where size is not the variable; exceptions: `ints` 15 KiB, `ints-extremes` 2 KiB, `wide-1000` 27 KiB, `long-string` 256 KiB, `enums` 64 KiB, `nested-*` 2–214 KiB, `pretty-json-large` 122 KiB) | `ints` (every digit count, 1–19 signed and 1–20 unsigned), `ints-extremes` (each digit count's boundaries, `i64::MIN`, `i64::MAX`, `u64::MAX`), `floats` (uniform over finite `f64` bit patterns), `f32s` (the same over `f32`), `escapes` (about 27 % escaped bytes), `unicode` (raw CJK, Hangul, Cyrillic, Greek, emoji), `long-string` (one 256 KiB ASCII string with nothing to escape), `nested-{100,127,129,10000}` (object depth either side of serde_json's limit of 128), `wide-1000` (a 1 000-key map), `array-small` (a long `Vec<Point>`), `enums` (64 KiB of records holding one externally, internally, adjacently tagged and untagged enum each), `pretty-json-large` (`json-large`'s value, pretty-printed), `floats-short` (prices with 2 decimals and coordinates with 6, as such values are usually written), `sparse` (records of mostly `null` options, booleans and empty arrays and maps), `defaults` (about half of each record's fields missing, filled by `#[serde(default)]`), `keys-long-int` (64-byte map keys, and integer map keys of every digit count) | one variable at a time |
| Rejections | `reject-truncated` (`json-large` cut at 90 %: a 400), `reject-shape` (`json-large` with one `id` a string: a 422) | what refusing a bad body costs; a backend that answers another status is judged wrong and not measured |
| Real-world schemas | `twitter-like` (800 KiB, non-ASCII written as `\u` escapes), `citm-like` (477 KiB, integer- and key-heavy), `canada-like` (2.1 MiB of full-precision floats) | the schemas of the three documents most JSON benchmarks use; the originals carry no licence that permits redistribution, so they are regenerated, not copied |
| Real-world variants | `raw-utf8` (`twitter-like` with its non-ASCII written raw), `pretty-twitter-like` (`twitter-like` pretty-printed), `skip-unknown` (`twitter-like`'s bytes into a struct that declares 8 of each status's 36 to 38 keys, so the rest must be skipped) | how a document is written, and how much of it the handler wants |

`pretty-json-large`, `pretty-twitter-like` and `raw-utf8` hold another workload's value, so their encode is that workload's encode byte for byte, and the rejections have nothing to encode; only their decode is measured.

Owned and borrowed forms coincide for the workloads with no strings: `ints`, `ints-extremes`, `floats`, `f32s`, `nested-*` and `array-small`.

### How the body arrives and how the response leaves

Decode input is `Bytes` in two forms:

- **`shared`**: one frame split off a larger buffer, as hyper hands out a single-frame body. It is not unique, so an in-place parser has to copy it.
- **`unique`**: a fresh buffer, as a multi-frame body is after collecting.

Encoding is measured two ways: `to_vec` into a new `Vec`, and `to_writer` into a reused `Vec` that already has capacity.

### Conformance

The differential suite is `crates/conformance`. It compares every backend with serde_json in the same build (so `serde_json+float_roundtrip`, which is serde_json, passes rule 2 as the reference; Miri still runs on it), and runs at every variant, because SIMD paths differ between variants. It also runs once with serde_json's `arbitrary_precision` unified in.

| Section | What | Gates? |
| --- | --- | --- |
| grammar | JSONTestSuite `test_parsing` (95 `y_`, 188 `n_`, 35 `i_`, vendored with licence and digests), decoded into `Value` and into `IgnoredAny` (which catches lazy skipping that never validates), for both arrivals | `y_`/`n_` yes; `i_` recorded |
| status | 400/422 for every `n_` case, wrong-shape inputs, and every truncation of the three Kynos bodies | yes |
| strings | lone and paired surrogates, invalid UTF-8, into `String`, `&str` and `Cow` | yes |
| structs | duplicate fields, `deny_unknown_fields`, an unknown member that must still be valid JSON (an invalid escape inside it), whitespace after the document | yes |
| depth | 100 up to 1 000 000 levels, run in a subprocess on 2 MiB (tokio's worker stack) and 8 MiB stacks | a crash gates; the limit itself is recorded |
| numbers | edge values; 1 000 000 random `f64` and 1 000 000 random `f32` bit patterns; exponents with leading zeros (`1e007`). A decode passes if it is correctly rounded (the truth is `str::parse`) or bit-identical to serde_json's own decode in the same build. Encoders must round-trip exactly. | yes |
| attributes | `flatten`, `untagged`, internally, adjacently and externally tagged enums, `alias`, `rename_all`, `default`, `skip`, `Option` | yes |
| value_targets | decoding into `serde_json::Value` and `Map` | yes; under `arbitrary_precision` only a difference in how a number is held (the same value) is excused |
| encoding | control characters, `/`, `<`, U+2028, U+2029, NaN, ±∞ | invalid or different output yes; byte differences, and refusing to encode NaN or ±∞ (serde_json silently writes `null`), recorded |
| encode_diff | every workload, byte for byte against serde_json; differing bytes are compared for meaning through the workload's own types (an `f32` written with more digits reads back the same `f32`) | semantic differences yes |

Beyond the differential suite:

- **Fuzzing**: differential fuzzing (cargo-fuzz, `nightly-2026-09-25`, AddressSanitizer), at `v3` for every candidate and also at `portable` for sonic-rs, whose fallback scanner is a separate implementation. `v3` runs the SIMD paths `native` takes on the measured host: no backend has an AVX-512 path, and `doctor` checks which path each build takes. It covers arbitrary bytes into `Value`, arbitrary bytes into an attribute-heavy struct (owned, and borrowed from a shared body, so an in-place parser's copy is fuzzed too), and arbitrary values round-tripped through both encoders, against serde_json built with `float_roundtrip`, so a candidate's floats are compared with the correctly rounded value. Each entry point is fuzzed on its own (`simd-json-buffers` and the in-place `-mut` entries included), and every target runs for 30 minutes per entry point and build configuration on two libFuzzer workers, and continues past each divergence, so the whole 30 minutes is fuzzed and nothing behind the first finding is missed. Each run records how many inputs crashed, the smallest of them, its assertion, and any memory error the sanitizer reported. A divergence fails rule 2 (conformance); a sanitizer report fails rule 3 (soundness).
- **Miri**: runs a subset of the suite wherever the backend's intrinsics allow it.

A backend that disagrees silently with serde_json on anything that gates fails. Two rules decide what counts as a disagreement:

- **Untyped values are judged against the input.** A decoded `Value` passes if it equals serde_json's, or if it holds exactly what the input text says, with numbers compared as values. Floats count as equal when both are correctly rounded, and `-0` counts as equal to the integer `0`. Without this rule, every backend that rounds correctly would fail wherever serde_json's default parser does not. Differences allowed by this rule are still recorded: every section counts all of its differences and keeps the first 50, all exported to `report/data/`.
- **Typed targets are strict.** An `f64` field decoded from `-0.0` must keep its sign.

Each adapter maps its crate's errors to 400/422 as faithfully as that crate's error type allows, and says in a comment what it cannot distinguish.

### End to end

`crates/e2e` is a hyper 1.x server that reads each body the way a hyper-based framework does (`collect().to_bytes()`) and uses one backend per build. Its routes are `POST /echo`, `GET /json/large`, `POST /json/large` and `GET /json/small`. A *floor* build serves the same routes with no JSON: it collects request bodies as every build does, discards them, and returns pre-encoded bytes. The 64 KiB `POST /json/large` body arrives in several frames through the counted in-memory connection (so the decode reads a fresh, unique buffer), but over loopback almost always in one (a shared frame, which an in-place parser copies).

- **Counted**: callgrind per request over an in-memory connection, at `portable` and `v3`; the rule reads `v3`. The codec's share of a request is (backend − floor) ÷ backend. That share bounds what swapping the codec can gain end to end. A timed gain larger than the count predicts is treated as a measurement error, not a result.
- **Timed**: loopback, with the server on cores 0–3 and oha 1.16.0 on every other core that is not an SMT sibling of those. Closed-loop throughput is measured at 64 connections, then open-loop p99 at 70 % of serde_json's throughput (with coordinated-omission correction), where serde_json's throughput is the median of three closed-loop calibration runs. Each measurement is 30 s, the backends run 5 times in shuffled order, and serde_json runs twice as an A/A control. A run in which any response is not the route's status (200, or 204 for `POST /json/large`), or any request fails, is an error, and so is an open-loop run that achieves less than 99 % of its offered rate.

### Timed measurements and trust

Hardware cycles, wall-clock time and end-to-end timing all carry a trust class:

- **`solo`**: a shared machine, with the sample checks above.
- **`canonical`**: a quiet, dedicated machine, stated with `--trust canonical`, which is refused unless the 1-minute load average is at most 1.0 and the CPU governor is `performance`.

A pass under rule 1 or rule 4 that rests on `solo` timing is reported as provisional. Wall-clock time per operation uses the hardware-cycles protocol, with 100 samples of 20 ms after a 3 s warm-up in place of counters. It replaces criterion: rejecting a disturbed sample and interleaving processes across rounds are the two things the protocol needs, and neither can be done inside one criterion run.

### Hosts

| Host | Counted (callgrind) | Hardware cycles | Timed |
| --- | --- | --- | --- |
| AMD Ryzen 7 7800X3D (Zen 4), Linux | `portable`, `v3` in the container (valgrind cannot run AVX-512, so not `native`) | `native` | `native` |
| GitHub `ubuntu-24.04` | reproducibility check only: counts must match the committed ones within 0.5 % | no | no |

There is no Intel machine, so no Intel hardware-cycle or wall-clock number exists.

### Decision rule

*Dominance* is ε-dominance with ε = 5 %. One entry dominates another when it is no more than 5 % worse on every axis and more than 5 % better on at least one. On a CPU axis, read in hardware cycles, the margin is ε or the A/A band at that cell, whichever is wider: a difference serde_json shows against itself is noise. The axes are:

- decode cycles
- encode cycles
- decode heap bytes
- encode heap bytes
- `.text` added
- clean release compile time

Decode-only entries are compared on the decode axes alone, and an encoder is only ever compared with other encoders. The pool excludes any entry that has failed conformance: a codec that returns wrong answers cannot decide which codecs are faster.

A backend is **recommended for Kynos** only if all four hold:

1. **Frontier.** On at least one Kynos shape, it is non-dominated among eligible entries at x86-64 `native` and more than the margin better than serde_json on decode or encode cycles or heap. Every cycles advantage, in a win or in dominance, must agree with the counts: one entry's cycles advantage over another counts only if, at `v3`, its callgrind estimated cycles for the same cell are no more than 5 % above the other's. Without an A/A control for a Kynos cell the rule is pending, and it is provisional unless the cycles are `canonical`.
2. **Conformance.** No gating disagreement at any variant, no fuzz divergence, and no undefined behaviour under Miri.
3. **Soundness.** No advisory affecting the pinned version, no open soundness issue, and no memory error reported by the sanitizer while fuzzing.
4. **End to end.** At least 10 % better throughput or p99 on `echo-post` or `json-large`, outside the A/A band, and consistent with the counted codec share: no more throughput than the count predicts, and a p99 gain only where the count predicts a gain of more than 5 % (a p99 under load is not proportional to a request's cost, so it has no tighter bound). The A/A control and the prediction must both exist, or the rule is pending.

Separately from Kynos, a library **deserves to exist** if either of these holds. Neither eligibility nor conformance is required here: a library can be worth having for what it does without being a drop-in replacement, and its frontier is drawn among every entry that passes conformance.

- It is on the frontier, with a win under the same margins, for *any* workload at `native`.
- It offers a capability serde_json lacks. The capability is stated with its evidence; any figure in it is computed from the results (`results/notes.toml` names the query, the report fills in the number).

### Verification

The results stand only after these checks:

1. **The harness is correct.** The unit and integration tests (`mise run test`).
2. **The results are one measurement of the committed code.** Every result file is stamped with a fingerprint of the measured code: a hash of every tracked file at the commit, excluding the report and the code that renders it (`xtask/src/report*`), this README, CI and the results. The report refuses a file stamped with another fingerprint or from a dirty checkout. It also refuses a per-operation table missing a row of the matrix, and a note whose figure the results cannot answer. A partial rerun after a change to measured code is therefore caught, not mixed in.
3. **The counts reproduce.** callgrind has no noise, so a recount on the host that took the committed counts, from an empty `target/`, must return every count unchanged. `mise run count:check` recounts the whole matrix at zero tolerance and records the result in `results/callgrind/recount.json`, and the report refuses results without a recount of every committed count. That holds only because every counted process runs with the same environment, nothing but a fixed `PATH`: the environment sits on the process's stack, and its length alone moves some counts (`xtask/src/counted.rs`). On another x86-64 host, CI's `count --check` holds the Kynos shapes to 0.5 %, which covers differences in libc and kernel.
4. **The harness can fail.** A faulty backend is caught exactly where it is faulty, and nowhere else:
   - An `n_` file accepted, a wrong shape answered 400, and floats one bit off each fail conformance in their own section. Floats written in a different format appear in `encode_diff` as a difference that does not gate (`crates/conformance/tests/harness.rs`).
   - An adapter that copies its input once more shows exactly one more allocation, of the input's size, and that much more peak. Under callgrind it costs the copy's instructions (`crates/cell/src/sensitivity.rs`). The callgrind half needs valgrind, so it runs in the container rather than in `mise run test`: `mise run sensitivity`, which `mise run all` runs before measuring.
   - An adapter that does less work than it should, the kind of variant an optimizer could delete, is refused by the result check in every mode and never measured.
   - A cycles "win" inside the A/A band, or one the count contradicts, is not a win (`xtask/src/report/verdict.rs` tests).
5. **The commentary matches the report.** Every number quoted under [Findings](#findings) links to the report row it comes from, and `cargo xtask report --check` fails if that row does not hold it, or if a figure in Findings has no link.
6. **Nothing is left out.** Every requirement above maps to a task, a result file and a report table, or to a deviation listed here. Every row of every result file is exported to `report/data/`.

Deviations:

- `mise run test` and `mise run sensitivity` (checks 1 and 4) leave no result file: `mise run all` stops if either fails.
- End-to-end counts are not exactly reproducible from an empty `target/`, so the zero-tolerance recount covers the per-operation counts only. The end-to-end counts feed only the codec share and rule 4's prediction.
- A stage that was not measured must be declared, with its reason, in `results/skipped.toml`; the report states every such reason under Limitations.

### Reproduce

```sh
mise install              # toolchain, nextest, convco, oha, hk, cargo-fuzz
mise run all              # every measurement and check, on one commit, then the report (hours)
SJC_TRUST=canonical mise run all   # the same on a quieted host: timing that can decide
cargo xtask report        # regenerate report/ from results/
cargo xtask report --check
```

Individual tasks are listed by `cargo xtask`. Every result file under `results/` records its commit, measured-code fingerprint, host, compiler, governor, load and container image; the report's [provenance table](report/README.md#provenance) lists them.

## Findings

<!-- findings: written from report/ after the rerun -->

## Licence

MIT or Apache-2.0, at your option. The vendored JSONTestSuite files are MIT (see `references/JSONTestSuite/LICENSE`).
