# serde-json-contenders

Every Rust web framework decodes and encodes typed JSON bodies, and nearly all of them do it with serde_json. Several libraries claim to do the same work faster, and their numbers are usually taken with the CPU tuned to the benchmark machine, parse into an untyped tree that no request handler uses, and sometimes switch on a serde_json option that slows the baseline down. This repository asks the question those numbers skip: if you replace serde_json behind a server's typed JSON body, and change nothing else, does anything get better by enough to matter? A library passes only if it is measurably better on some axis without being worse on others, returns the same values and the same errors, is sound, and still wins when the codec is one part of a whole HTTP request. The answer applies to any serde-based server, and one framework, Kynos, uses it to decide which codecs it will support.

## Licence

MIT or Apache-2.0, at your option.
