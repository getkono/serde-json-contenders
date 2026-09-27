//! One operation on one workload through one backend, and its inputs.

use std::hint::black_box;
use std::marker::PhantomData;

use bytes::Bytes;
use codecs::body::Arrival;
use codecs::{Backend, Body};
use payloads::Workload;
use serde_json::{Value, json};

use crate::modes;
use crate::verify::{Verdict, compare};

/// Which operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Bytes to `T`.
    Decode,
    /// `T` to a fresh `Vec` (`to_vec`).
    Encode,
    /// `T` into a reused, pre-sized `Vec` (`to_writer`).
    EncodeInto,
}

impl Op {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "decode" => Some(Self::Decode),
            "encode" => Some(Self::Encode),
            "encode-into" => Some(Self::EncodeInto),
            _ => None,
        }
    }
}

/// Which form of the workload type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// `String` fields.
    Owned,
    /// `Cow<'de, str>` fields.
    Borrowed,
}

impl Form {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owned" => Some(Self::Owned),
            "borrowed" => Some(Self::Borrowed),
            _ => None,
        }
    }
}

/// What was asked for on the command line.
#[derive(Debug, Clone)]
pub struct Spec {
    pub mode: String,
    pub op: Op,
    pub form: Form,
    pub arrival: Arrival,
    pub iters: Option<u64>,
}

/// A measurable operation.
pub trait Job {
    /// Make `n` operations' inputs ready. Never measured.
    fn prepare(&mut self, n: u64);
    /// Perform one operation on the next prepared input. This is what is
    /// measured, including dropping whatever it produced.
    fn run(&mut self);
    /// Run once and check the result.
    fn verify(&mut self) -> Verdict;
    /// Input or output size, in bytes.
    fn size(&self) -> usize;
}

/// Iterations for a counted run: enough to amortize per-call noise, bounded
/// so a large input's copies fit in memory.
#[must_use]
pub fn default_iters(size: usize) -> u64 {
    ((4 << 20) / size.max(1)).clamp(1, 1000) as u64
}

/// Resolves the backend, then the workload.
pub struct ByBackend<'a> {
    pub spec: Spec,
    pub workload: &'a str,
}

impl codecs::Visit for ByBackend<'_> {
    type Output = Option<Value>;

    fn visit<B: Backend>(self) -> Option<Value> {
        payloads::dispatch(
            self.workload,
            ByWorkload::<B> {
                spec: self.spec,
                _b: PhantomData,
            },
        )
    }
}

struct ByWorkload<B> {
    spec: Spec,
    _b: PhantomData<B>,
}

impl<B: Backend> payloads::Visit for ByWorkload<B> {
    type Output = Value;

    fn visit<W: Workload>(self) -> Value {
        let spec = self.spec;
        let header = json!({
            "backend": B::NAME,
            "crate": B::CRATE,
            "workload": W::NAME,
            "op": format!("{:?}", spec.op),
            "form": format!("{:?}", spec.form),
            "arrival": spec.arrival.name(),
        });
        let mut output = match (spec.op, spec.form) {
            (Op::Decode, Form::Owned) => modes::run(&spec, &mut DecodeOwned::<B, W>::new(spec.arrival)),
            (Op::Decode, Form::Borrowed) => modes::run(&spec, &mut DecodeBorrowed::<B, W>::new(spec.arrival)),
            (Op::Encode | Op::EncodeInto, _) if !B::ENCODES => json!({ "verdict": "unsupported" }),
            (Op::Encode, _) => modes::run(&spec, &mut Encode::<B, W>::new(false)),
            (Op::EncodeInto, _) => modes::run(&spec, &mut Encode::<B, W>::new(true)),
        };
        if let (Value::Object(out), Value::Object(head)) = (&mut output, header) {
            out.extend(head);
        }
        output
    }
}

/// The inputs a decode consumes.
struct Inputs {
    arrival: Arrival,
    /// For `Shared`: every input is a clone of this, which shares its
    /// allocation with `_rest` exactly as a single frame shares a connection
    /// buffer. For `Unique`: the source copies are made from.
    template: Bytes,
    _rest: Option<Bytes>,
    pool: Vec<Bytes>,
}

impl Inputs {
    fn new(arrival: Arrival, bytes: &[u8]) -> Self {
        let (template, rest) = arrival.materialize(bytes);
        Self {
            arrival,
            template,
            _rest: rest,
            pool: Vec::new(),
        }
    }

    fn prepare(&mut self, n: u64) {
        if self.arrival == Arrival::Unique {
            self.pool.clear();
            self.pool.extend((0..n).map(|_| Bytes::from(self.template.to_vec())));
        }
    }

    fn next(&mut self) -> Bytes {
        match self.arrival {
            Arrival::Shared => self.template.clone(),
            Arrival::Unique => self
                .pool
                .pop()
                .unwrap_or_else(|| unreachable!("prepare() sized the pool")),
        }
    }
}

struct DecodeOwned<B, W> {
    inputs: Inputs,
    size: usize,
    _p: PhantomData<(B, W)>,
}

impl<B: Backend, W: Workload> DecodeOwned<B, W> {
    fn new(arrival: Arrival) -> Self {
        let bytes = W::bytes();
        Self {
            size: bytes.len(),
            inputs: Inputs::new(arrival, &bytes),
            _p: PhantomData,
        }
    }
}

impl<B: Backend, W: Workload> Job for DecodeOwned<B, W> {
    fn prepare(&mut self, n: u64) {
        self.inputs.prepare(n);
    }

    fn run(&mut self) {
        let input = self.inputs.next();
        let _ = black_box(B::decode::<W::Owned>(black_box(input)));
    }

    fn verify(&mut self) -> Verdict {
        self.prepare(1);
        match B::decode::<W::Owned>(self.inputs.next()) {
            Ok(value) => compare(&to_value(&value), &to_value(&W::value())),
            Err(failure) => Verdict::Rejected(failure.to_string()),
        }
    }

    fn size(&self) -> usize {
        self.size
    }
}

struct DecodeBorrowed<B, W> {
    inputs: Inputs,
    size: usize,
    _p: PhantomData<(B, W)>,
}

impl<B: Backend, W: Workload> DecodeBorrowed<B, W> {
    fn new(arrival: Arrival) -> Self {
        let bytes = W::bytes();
        Self {
            size: bytes.len(),
            inputs: Inputs::new(arrival, &bytes),
            _p: PhantomData,
        }
    }
}

impl<B: Backend, W: Workload> Job for DecodeBorrowed<B, W> {
    fn prepare(&mut self, n: u64) {
        self.inputs.prepare(n);
    }

    fn run(&mut self) {
        let mut body = Body::new(black_box(self.inputs.next()));
        let _ = black_box(B::decode_borrowed::<W::Borrowed<'_>>(&mut body));
    }

    fn verify(&mut self) -> Verdict {
        self.prepare(1);
        let mut body = Body::new(self.inputs.next());
        match B::decode_borrowed::<W::Borrowed<'_>>(&mut body) {
            Ok(value) => compare(&to_value(&value), &to_value(&W::value())),
            Err(failure) => Verdict::Rejected(failure.to_string()),
        }
    }

    fn size(&self) -> usize {
        self.size
    }
}

struct Encode<B, W: Workload> {
    value: W::Owned,
    into: bool,
    out: Vec<u8>,
    size: usize,
    _p: PhantomData<B>,
}

impl<B: Backend, W: Workload> Encode<B, W> {
    fn new(into: bool) -> Self {
        let value = W::value();
        Self {
            size: payloads::encode_canonical(&value).len(),
            value,
            into,
            out: Vec::new(),
            _p: PhantomData,
        }
    }
}

impl<B: Backend, W: Workload> Job for Encode<B, W> {
    fn prepare(&mut self, _: u64) {
        if self.into && self.out.capacity() < self.size {
            self.out.reserve(self.size * 2);
        }
    }

    fn run(&mut self) {
        if self.into {
            let _ = black_box(B::encode_into(black_box(&self.value), &mut self.out));
        } else {
            let _ = black_box(B::encode(black_box(&self.value)));
        }
    }

    fn verify(&mut self) -> Verdict {
        let encoded = if self.into {
            self.prepare(1);
            B::encode_into(&self.value, &mut self.out).map(|()| self.out.clone())
        } else {
            B::encode(&self.value)
        };
        match encoded {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(parsed) => {
                    let reference = serde_json::from_slice::<Value>(&payloads::encode_canonical(&self.value))
                        .unwrap_or_else(|e| unreachable!("baseline re-parses its own output: {e}"));
                    compare(&parsed, &reference)
                }
                Err(e) => Verdict::Wrong(format!("output is not JSON: {e}")),
            },
            Err(failure) => Verdict::Rejected(failure.to_string()),
        }
    }

    fn size(&self) -> usize {
        self.size
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|e| unreachable!("a decoded value re-serializes: {e}"))
}
