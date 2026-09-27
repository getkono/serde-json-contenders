//! A small slice of the suite, for Miri to look for undefined behaviour in
//! the backends' decoders and encoders.
//!
//! `MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri test -p conformance
//! --test miri --features <F>`. Nothing about agreement is asserted: that is
//! the suite's job, and here only Miri's verdict matters. No subprocesses and
//! no custom stacks, which Miri cannot run. Also runs under plain
//! `cargo test`, so it keeps compiling.

use codecs::body::Arrival;
use codecs::{Backend, Visit};
use conformance::corpus::{Corpus, Expect, default_dir};
use conformance::outcome::{AsValue, owned};
use conformance::{numbers, strings};

/// Random floats per backend: enough to cross every number path.
const SAMPLES: usize = 1_000;

struct Smoke<'a>(&'a Corpus);

impl Visit for Smoke<'_> {
    type Output = ();

    fn visit<B: Backend>(self) {
        // Deep inputs are left to the depth section, which runs each in a
        // subprocess: a backend without a depth limit overflows the stack.
        for case in self
            .0
            .cases
            .iter()
            .filter(|case| case.expect != Expect::Either && nesting(&case.bytes) <= 128)
        {
            let _ = owned::<B, AsValue>(&case.bytes, Arrival::Unique);
        }
        let _ = strings::run::<B>();
        let _ = numbers::run::<B>(SAMPLES);
    }
}

#[test]
fn every_backend_decodes_and_encodes_without_undefined_behaviour() {
    // Digests are not verified: the SHA-256 implementation's CPU feature
    // detection is outside what Miri can run. The counts still are.
    let corpus = Corpus::load_unverified(&default_dir()).expect("the vendored corpus loads");
    let mut backends: Vec<&str> = codecs::compiled().into_iter().skip(1).collect();
    if backends.is_empty() {
        // A serde_json-only build still exercises the target.
        backends = codecs::compiled();
    }
    for name in backends {
        assert!(
            codecs::dispatch(name, Smoke(&corpus)).is_some(),
            "{name} is compiled but not dispatchable"
        );
    }
}

/// The deepest bracket nesting in `bytes`, outside strings.
fn nesting(bytes: &[u8]) -> usize {
    let (mut depth, mut max, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for &b in bytes {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b'[' || b == b'{' {
            depth += 1;
            max = max.max(depth);
        } else if b == b']' || b == b'}' {
            depth = depth.saturating_sub(1);
        }
    }
    max
}
