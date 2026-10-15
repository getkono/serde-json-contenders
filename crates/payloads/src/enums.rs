//! `enums`: records mixing serde's four enum representations, as an API of
//! events and commands carries them.
//!
//! The externally tagged enum is a plain visitor walk; the adjacently tagged
//! one reads `tag` then `content` in order; the internally tagged and untagged
//! ones cannot know their variant until they have read further than its
//! start, so serde buffers them through its private `Content` tree first. That
//! buffering, and whether a backend can still borrow through it, is the
//! variable.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::families::fill_to;
use crate::{Group, Rng, Workload};

/// Target body size: large enough that per-record buffering dominates.
pub const ENUMS_BYTES: usize = 64 * 1024;

/// Externally tagged (serde's default): `"stop"` or `{"variant": ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Command {
    Stop,
    Resize(u32),
    Move(i32, i32),
    Rename { from: String, to: String },
}

/// [`Command`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum CommandRef<'a> {
    Stop,
    Resize(u32),
    Move(i32, i32),
    Rename {
        #[serde(borrow)]
        from: Cow<'a, str>,
        #[serde(borrow)]
        to: Cow<'a, str>,
    },
}

/// Internally tagged: `{"type": "variant", ...fields}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Event {
    Created { id: u64, name: String, public: bool },
    Deleted { id: u64 },
    Moved { id: u64, from: String, to: String },
    Heartbeat,
}

/// [`Event`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum EventRef<'a> {
    Created {
        id: u64,
        #[serde(borrow)]
        name: Cow<'a, str>,
        public: bool,
    },
    Deleted {
        id: u64,
    },
    Moved {
        id: u64,
        #[serde(borrow)]
        from: Cow<'a, str>,
        #[serde(borrow)]
        to: Cow<'a, str>,
    },
    Heartbeat,
}

/// Adjacently tagged: `{"tag": "variant", "content": ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tag", content = "content", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Envelope {
    Text(String),
    Count(u64),
    Pair(u32, u32),
    Login { user: String, admin: bool },
    Ping,
}

/// [`Envelope`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tag", content = "content", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum EnvelopeRef<'a> {
    Text(#[serde(borrow)] Cow<'a, str>),
    Count(u64),
    Pair(u32, u32),
    Login {
        #[serde(borrow)]
        user: Cow<'a, str>,
        admin: bool,
    },
    Ping,
}

/// Untagged: whichever variant first accepts the value, in declaration order.
/// `List` precedes the structs because a derived struct also accepts a
/// sequence: `[3,5]` would otherwise come back as a `Point`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(missing_docs)]
pub enum Payload {
    Nothing,
    Number(i64),
    Text(String),
    List(Vec<u64>),
    Error { code: u32, message: String },
    Point { x: i32, y: i32 },
}

/// [`Payload`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(missing_docs)]
pub enum PayloadRef<'a> {
    Nothing,
    Number(i64),
    Text(#[serde(borrow)] Cow<'a, str>),
    List(Vec<u64>),
    Error {
        code: u32,
        #[serde(borrow)]
        message: Cow<'a, str>,
    },
    Point {
        x: i32,
        y: i32,
    },
}

/// One record: one enum of each representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Record {
    pub seq: u64,
    pub command: Command,
    pub event: Event,
    pub envelope: Envelope,
    pub payload: Payload,
}

/// [`Record`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct RecordRef<'a> {
    pub seq: u64,
    #[serde(borrow)]
    pub command: CommandRef<'a>,
    #[serde(borrow)]
    pub event: EventRef<'a>,
    #[serde(borrow)]
    pub envelope: EnvelopeRef<'a>,
    #[serde(borrow)]
    pub payload: PayloadRef<'a>,
}

const NAMES: &[&str] = &["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel"];

fn name(rng: &mut Rng) -> String {
    format!("{}-{}", rng.pick(NAMES), rng.below(1000))
}

fn record(rng: &mut Rng, seq: u64) -> Record {
    let command = match rng.below(4) {
        0 => Command::Stop,
        1 => Command::Resize(rng.below(4096) as u32),
        2 => Command::Move(rng.below(2001) as i32 - 1000, rng.below(2001) as i32 - 1000),
        _ => Command::Rename {
            from: name(rng),
            to: name(rng),
        },
    };
    let event = match rng.below(4) {
        0 => Event::Created {
            id: rng.below(1 << 40),
            name: name(rng),
            public: rng.below(2) == 0,
        },
        1 => Event::Deleted { id: rng.below(1 << 40) },
        2 => Event::Moved {
            id: rng.below(1 << 40),
            from: name(rng),
            to: name(rng),
        },
        _ => Event::Heartbeat,
    };
    let envelope = match rng.below(5) {
        0 => Envelope::Text(name(rng)),
        1 => Envelope::Count(rng.below(1 << 32)),
        2 => Envelope::Pair(rng.below(1000) as u32, rng.below(1000) as u32),
        3 => Envelope::Login {
            user: name(rng),
            admin: rng.below(8) == 0,
        },
        _ => Envelope::Ping,
    };
    let payload = match rng.below(6) {
        0 => Payload::Nothing,
        1 => Payload::Number(rng.below(1 << 40) as i64 - (1 << 39)),
        2 => Payload::Text(name(rng)),
        3 => Payload::Error {
            code: 400 + rng.below(200) as u32,
            message: name(rng),
        },
        4 => Payload::Point {
            x: rng.below(2001) as i32 - 1000,
            y: rng.below(2001) as i32 - 1000,
        },
        _ => Payload::List((0..rng.below(5)).map(|_| rng.below(1000)).collect()),
    };
    Record {
        seq,
        command,
        event,
        envelope,
        payload,
    }
}

/// `enums`: ~64 KiB of [`Record`]s.
#[derive(Debug)]
pub struct Enums;

impl Workload for Enums {
    const NAME: &'static str = "enums";
    const GROUP: Group = Group::Family;
    type Owned = Vec<Record>;
    type Borrowed<'de> = Vec<RecordRef<'de>>;

    fn value() -> Vec<Record> {
        let mut rng = Rng::new(0xe9a5);
        let mut seq = 0;
        fill_to(ENUMS_BYTES, || {
            seq += 1;
            record(&mut rng, seq)
        })
    }
}
