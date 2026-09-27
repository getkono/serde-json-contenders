//! One decode, run so that nothing a backend does short of crashing the
//! process can take the suite down, reduced to a comparable [`Outcome`].

use std::any::Any;
use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};

use codecs::body::Arrival;
use codecs::{Backend, Body, Failure};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value, json};

/// Both ways a server receives a body; in-place parsers behave differently
/// on each.
pub const ARRIVALS: [Arrival; 2] = [Arrival::Unique, Arrival::Shared];

/// What a decode did.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Accepted, with the decoded value as serde_json sees it.
    Accept(Value),
    /// Rejected.
    Reject(Failure),
    /// Panicked, with the panic message.
    Panic(String),
}

impl Outcome {
    /// From a guarded decode.
    #[must_use]
    pub fn from_result(result: Result<Result<Value, Failure>, String>) -> Self {
        match result {
            Ok(Ok(value)) => Self::Accept(value),
            Ok(Err(failure)) => Self::Reject(failure),
            Err(panic) => Self::Panic(panic),
        }
    }

    /// Whether the input was accepted.
    #[must_use]
    pub fn is_accept(&self) -> bool {
        matches!(self, Self::Accept(_))
    }

    /// The status a server answers with; 200 for an accepted body, none for a
    /// panic.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Accept(_) => Some(200),
            Self::Reject(failure) => Some(failure.status()),
            Self::Panic(_) => None,
        }
    }

    /// The report's rendering.
    #[must_use]
    pub fn describe(&self) -> Value {
        match self {
            Self::Accept(value) => json!({ "accept": abbreviate(value) }),
            Self::Reject(failure) => json!({
                "reject": { "status": failure.status(), "class": failure.class, "message": failure.message }
            }),
            Self::Panic(message) => json!({ "panic": message }),
        }
    }
}

/// Longest rendering of a value kept as JSON in the report.
pub const RENDER_BYTES: usize = 400;
/// Deepest value kept as JSON in the report, well inside serde_json's own
/// recursion limit so the report stays readable by serde_json.
pub const RENDER_DEPTH: usize = 32;

/// A value as the report shows it: itself if small and shallow, otherwise a
/// string of its truncated text.
#[must_use]
pub fn abbreviate(value: &Value) -> Value {
    let text = value.to_string();
    if text.len() <= RENDER_BYTES && depth(value) <= RENDER_DEPTH {
        return value.clone();
    }
    let mut end = RENDER_BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Value::String(format!(
        "{}... ({} bytes, depth {})",
        &text[..end],
        text.len(),
        depth(value)
    ))
}

fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(members) => 1 + members.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Whether two outcomes agree: same accept/reject, the same value exactly,
/// and the same status. A panic agrees with nothing.
#[must_use]
pub fn agree(reference: &Outcome, backend: &Outcome) -> bool {
    match (reference, backend) {
        (Outcome::Accept(a), Outcome::Accept(b)) => same_value(a, b),
        (Outcome::Reject(a), Outcome::Reject(b)) => a.status() == b.status(),
        _ => false,
    }
}

/// Whether an accepted `Value` holds exactly what the input text says, with
/// numbers compared as values ([`crate::exact::numbers_equal`]: floats by
/// correct rounding, `-0` equal to `0` as integers). A backend that is
/// faithful to its input is not penalized for differing from serde_json's
/// own rounding or number representation; the differences are still listed.
#[must_use]
pub fn faithful(input: &[u8], backend: &Outcome) -> bool {
    match (backend, crate::exact::parse(input)) {
        (Outcome::Accept(value), Ok(node)) => crate::exact::matches_value(&node, value),
        _ => false,
    }
}

/// Exact `Value` equality: floats by bit pattern, and integers and floats
/// never equal to each other. Under `arbitrary_precision` numbers are text,
/// so they compare by [`crate::exact::numbers_equal`] instead.
#[must_use]
pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => same_number(x, y),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_value(x, y)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|((kx, x), (ky, y))| kx == ky && same_value(x, y))
        }
        _ => false,
    }
}

fn same_number(x: &Number, y: &Number) -> bool {
    if cfg!(feature = "arbitrary-precision") {
        return crate::exact::numbers_equal(&x.to_string(), &y.to_string());
    }
    match (x.is_f64(), y.is_f64()) {
        (true, true) => x.as_f64().map(f64::to_bits) == y.as_f64().map(f64::to_bits),
        (false, false) => x.as_u64() == y.as_u64() && x.as_i64() == y.as_i64(),
        _ => false,
    }
}

thread_local! {
    static GUARDED: Cell<bool> = const { Cell::new(false) };
}

/// Run `f`, turning a panic into its message.
pub fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    let outer = GUARDED.replace(true);
    let result = catch_unwind(AssertUnwindSafe(f)).map_err(|payload| panic_message(&*payload));
    GUARDED.set(outer);
    result
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

/// Silence the default panic output for panics [`guarded`] catches, which
/// are findings, while keeping it for panics in the harness itself.
pub fn quiet_guarded_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !GUARDED.get() {
            default(info);
        }
    }));
}

/// A type decoded into, and how its result is compared.
pub trait Target {
    /// The decoded type, possibly borrowing from the input.
    type Out<'de>: Deserialize<'de>;
    /// The result, as a `Value` compared with [`same_value`].
    fn view(out: &Self::Out<'_>) -> Value;
}

/// Decode owned: `Backend::decode` on the body as it arrives.
#[must_use]
pub fn owned<B: Backend, K: Target>(input: &[u8], arrival: Arrival) -> Outcome
where
    K::Out<'static>: DeserializeOwned,
{
    let (bytes, _guard) = arrival.materialize(input);
    Outcome::from_result(guarded(|| B::decode::<K::Out<'static>>(bytes).map(|out| K::view(&out))))
}

/// Decode borrowed: `Backend::decode_borrowed` on the body as it arrives.
#[must_use]
pub fn borrowed<B: Backend, K: Target>(input: &[u8], arrival: Arrival) -> Outcome {
    let (bytes, _guard) = arrival.materialize(input);
    let mut body = Body::new(bytes);
    Outcome::from_result(guarded(|| {
        B::decode_borrowed::<K::Out<'_>>(&mut body).map(|out| K::view(&out))
    }))
}

/// A decode path, as a plain function so cases can be tabulated.
pub type Decode = fn(&[u8], Arrival) -> Outcome;

/// `serde_json::Value`.
#[derive(Debug)]
pub struct AsValue;

impl Target for AsValue {
    type Out<'de> = Value;
    fn view(out: &Value) -> Value {
        out.clone()
    }
}

/// `serde_json::Map<String, Value>`.
#[derive(Debug)]
pub struct AsMap;

impl Target for AsMap {
    type Out<'de> = Map<String, Value>;
    fn view(out: &Map<String, Value>) -> Value {
        Value::Object(out.clone())
    }
}

/// `serde::de::IgnoredAny`: validation only.
#[derive(Debug)]
pub struct Ignored;

impl Target for Ignored {
    type Out<'de> = IgnoredAny;
    fn view(_: &IgnoredAny) -> Value {
        Value::Null
    }
}

/// `&'de str`: only a backend that can hand out a slice of the input can
/// produce one.
#[derive(Debug)]
pub struct Str;

impl Target for Str {
    type Out<'de> = &'de str;
    fn view(out: &&str) -> Value {
        Value::String((*out).to_owned())
    }
}

/// Any owned type, compared through `serde_json::to_value`.
#[derive(Debug)]
pub struct Typed<T>(PhantomData<T>);

impl<T: DeserializeOwned + Serialize> Target for Typed<T> {
    type Out<'de> = T;
    fn view(out: &T) -> Value {
        to_value(out)
    }
}

/// A workload's borrowed form, compared through `serde_json::to_value`.
#[derive(Debug)]
pub struct BorrowedForm<W>(PhantomData<W>);

impl<W: payloads::Workload> Target for BorrowedForm<W> {
    type Out<'de> = W::Borrowed<'de>;
    fn view(out: &W::Borrowed<'_>) -> Value {
        to_value(out)
    }
}

/// `serde_json::to_value`, with a failure rendered rather than raised.
#[must_use]
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|error| json!({ "unserializable": error.to_string() }))
}
