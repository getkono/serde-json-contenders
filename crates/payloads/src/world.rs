//! Schema-equivalents of three documents every JSON benchmark uses.
//!
//! The originals carry no licence that permits redistribution, so these are
//! generated from the same schemas, within a factor of four of the original size, with the same
//! character: `twitter-like` is string-heavy with `\u`-escaped non-ASCII,
//! `citm-like` is integer- and key-heavy, `canada-like` is float-heavy.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Group, Rng, Workload, ascii_escape, encode_canonical};

const WORDS: &[&str] = &[
    "the",
    "request",
    "server",
    "json",
    "codec",
    "latency",
    "throughput",
    "stream",
    "buffer",
    "parse",
    "encode",
    "struct",
    "field",
    "value",
    "array",
    "object",
    "number",
    "string",
];
const JAPANESE: &[&str] = &[
    "東京",
    "日本語",
    "こんにちは",
    "ありがとう",
    "テスト",
    "ラーメン",
    "桜",
    "新幹線",
    "猫",
    "天気",
];

fn sentence(rng: &mut Rng, words: usize) -> String {
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        if rng.below(3) == 0 {
            out.push_str(rng.pick(JAPANESE));
        } else {
            out.push_str(rng.pick(WORDS));
        }
    }
    out
}

macro_rules! twin {
    ($(#[$m:meta])* $owned:ident / $borrowed:ident { $($(#[$fm:meta])* $f:ident : $t:ty => $bt:ty),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[allow(missing_docs)]
        pub struct $owned { $($(#[$fm])* pub $f: $t),* }
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[allow(missing_docs)]
        pub struct $borrowed<'a> { $($(#[$fm])* #[serde(borrow)] pub $f: $bt),* }
    };
}

// `#[serde(borrow)]` on a field with no lifetime is rejected, so borrowed twins
// only mark string-bearing fields; the rest are declared on plain structs.

/// A Twitter user, abridged to the fields that shape the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct User {
    pub id: u64,
    pub id_str: String,
    pub name: String,
    pub screen_name: String,
    pub location: String,
    pub description: String,
    pub url: Option<String>,
    pub protected: bool,
    pub followers_count: u32,
    pub friends_count: u32,
    pub listed_count: u32,
    pub created_at: String,
    pub favourites_count: u32,
    pub utc_offset: Option<i32>,
    pub time_zone: Option<String>,
    pub geo_enabled: bool,
    pub verified: bool,
    pub statuses_count: u32,
    pub lang: String,
    pub profile_background_color: String,
    pub profile_image_url: String,
    pub default_profile: bool,
}

/// [`User`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct UserRef<'a> {
    pub id: u64,
    #[serde(borrow)]
    pub id_str: Cow<'a, str>,
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    #[serde(borrow)]
    pub screen_name: Cow<'a, str>,
    #[serde(borrow)]
    pub location: Cow<'a, str>,
    #[serde(borrow)]
    pub description: Cow<'a, str>,
    #[serde(borrow)]
    pub url: Option<Cow<'a, str>>,
    pub protected: bool,
    pub followers_count: u32,
    pub friends_count: u32,
    pub listed_count: u32,
    #[serde(borrow)]
    pub created_at: Cow<'a, str>,
    pub favourites_count: u32,
    pub utc_offset: Option<i32>,
    #[serde(borrow)]
    pub time_zone: Option<Cow<'a, str>>,
    pub geo_enabled: bool,
    pub verified: bool,
    pub statuses_count: u32,
    #[serde(borrow)]
    pub lang: Cow<'a, str>,
    #[serde(borrow)]
    pub profile_background_color: Cow<'a, str>,
    #[serde(borrow)]
    pub profile_image_url: Cow<'a, str>,
    pub default_profile: bool,
}

twin! {
    /// A hashtag entity.
    Hashtag / HashtagRef { text: String => Cow<'a, str> }
}

/// A status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Status {
    pub created_at: String,
    pub id: u64,
    pub id_str: String,
    pub text: String,
    pub source: String,
    pub truncated: bool,
    pub in_reply_to_status_id: Option<u64>,
    pub user: User,
    pub retweet_count: u32,
    pub favorite_count: u32,
    pub hashtags: Vec<Hashtag>,
    pub favorited: bool,
    pub retweeted: bool,
    pub lang: String,
}

/// [`Status`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct StatusRef<'a> {
    #[serde(borrow)]
    pub created_at: Cow<'a, str>,
    pub id: u64,
    #[serde(borrow)]
    pub id_str: Cow<'a, str>,
    #[serde(borrow)]
    pub text: Cow<'a, str>,
    #[serde(borrow)]
    pub source: Cow<'a, str>,
    pub truncated: bool,
    pub in_reply_to_status_id: Option<u64>,
    #[serde(borrow)]
    pub user: UserRef<'a>,
    pub retweet_count: u32,
    pub favorite_count: u32,
    #[serde(borrow)]
    pub hashtags: Vec<HashtagRef<'a>>,
    pub favorited: bool,
    pub retweeted: bool,
    #[serde(borrow)]
    pub lang: Cow<'a, str>,
}

/// The search response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Twitter {
    /// The statuses.
    pub statuses: Vec<Status>,
}

/// [`Twitter`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwitterRef<'a> {
    /// The statuses.
    #[serde(borrow)]
    pub statuses: Vec<StatusRef<'a>>,
}

/// `twitter-like`: ~800 KiB, string-heavy, non-ASCII written as `\u` escapes.
#[derive(Debug)]
pub struct TwitterLike;

impl Workload for TwitterLike {
    const NAME: &'static str = "twitter-like";
    const GROUP: Group = Group::World;
    type Owned = Twitter;
    type Borrowed<'de> = TwitterRef<'de>;

    fn value() -> Twitter {
        let mut rng = Rng::new(0x7417);
        let statuses = (0..700)
            .map(|i| {
                let id = 505_874_924_095_815_681 + rng.below(1 << 40);
                let user_id = rng.below(3_000_000_000);
                let text = sentence(&mut rng, 12);
                Status {
                    created_at: format!("Sun Aug 31 00:{:02}:{:02} +0000 2014", i % 60, rng.below(60)),
                    id,
                    id_str: id.to_string(),
                    text,
                    source: "<a href=\"https://example.invalid/app\" rel=\"nofollow\">client</a>".into(),
                    truncated: false,
                    in_reply_to_status_id: (rng.below(4) == 0).then(|| id - 1),
                    user: User {
                        id: user_id,
                        id_str: user_id.to_string(),
                        name: sentence(&mut rng, 2),
                        screen_name: format!("user_{user_id:x}"),
                        location: sentence(&mut rng, 1),
                        description: sentence(&mut rng, 10),
                        url: (rng.below(2) == 0).then(|| format!("https://example.invalid/{user_id}")),
                        protected: false,
                        followers_count: rng.below(100_000) as u32,
                        friends_count: rng.below(5_000) as u32,
                        listed_count: rng.below(100) as u32,
                        created_at: "Mon Apr 26 06:01:55 +0000 2010".into(),
                        favourites_count: rng.below(10_000) as u32,
                        utc_offset: (rng.below(2) == 0).then_some(32_400),
                        time_zone: (rng.below(2) == 0).then(|| "Tokyo".into()),
                        geo_enabled: rng.below(2) == 0,
                        verified: false,
                        statuses_count: rng.below(50_000) as u32,
                        lang: "ja".into(),
                        profile_background_color: "C0DEED".into(),
                        profile_image_url: format!("https://example.invalid/img/{user_id}_normal.jpg"),
                        default_profile: rng.below(2) == 0,
                    },
                    retweet_count: rng.below(1000) as u32,
                    favorite_count: rng.below(1000) as u32,
                    hashtags: (0..rng.below(3))
                        .map(|_| Hashtag {
                            text: rng.pick(JAPANESE).to_string(),
                        })
                        .collect(),
                    favorited: false,
                    retweeted: false,
                    lang: "ja".into(),
                }
            })
            .collect();
        Twitter { statuses }
    }

    fn bytes() -> Vec<u8> {
        ascii_escape(&encode_canonical(&Self::value()))
    }
}

/// A seat category's areas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Area {
    pub area_id: u64,
    pub block_ids: Vec<u64>,
}

/// A seat category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct SeatCategory {
    pub areas: Vec<Area>,
    pub seat_category_id: u64,
}

/// A price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Price {
    pub amount: u64,
    pub audience_sub_category_id: u64,
    pub seat_category_id: u64,
}

/// A performance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Performance {
    pub event_id: u64,
    pub id: u64,
    pub logo: Option<String>,
    pub name: Option<String>,
    pub prices: Vec<Price>,
    pub seat_categories: Vec<SeatCategory>,
    pub seat_map_image: Option<String>,
    pub start: u64,
    pub venue_code: String,
}

/// [`Performance`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct PerformanceRef<'a> {
    pub event_id: u64,
    pub id: u64,
    #[serde(borrow)]
    pub logo: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pub name: Option<Cow<'a, str>>,
    pub prices: Vec<Price>,
    pub seat_categories: Vec<SeatCategory>,
    #[serde(borrow)]
    pub seat_map_image: Option<Cow<'a, str>>,
    pub start: u64,
    #[serde(borrow)]
    pub venue_code: Cow<'a, str>,
}

/// An event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Event {
    pub description: Option<String>,
    pub id: u64,
    pub logo: Option<String>,
    pub name: String,
    pub sub_topic_ids: Vec<u64>,
    pub subject_code: Option<String>,
    pub subtitle: Option<String>,
    pub topic_ids: Vec<u64>,
}

/// [`Event`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct EventRef<'a> {
    #[serde(borrow)]
    pub description: Option<Cow<'a, str>>,
    pub id: u64,
    #[serde(borrow)]
    pub logo: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    pub sub_topic_ids: Vec<u64>,
    #[serde(borrow)]
    pub subject_code: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pub subtitle: Option<Cow<'a, str>>,
    pub topic_ids: Vec<u64>,
}

/// The catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct Citm {
    pub area_names: BTreeMap<String, String>,
    pub events: BTreeMap<String, Event>,
    pub performances: Vec<Performance>,
    pub seat_category_names: BTreeMap<String, String>,
    pub topic_sub_topics: BTreeMap<String, Vec<u64>>,
    pub venue_names: BTreeMap<String, String>,
}

/// [`Citm`], borrowing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub struct CitmRef<'a> {
    #[serde(borrow)]
    pub area_names: BTreeMap<Cow<'a, str>, Cow<'a, str>>,
    #[serde(borrow)]
    pub events: BTreeMap<Cow<'a, str>, EventRef<'a>>,
    #[serde(borrow)]
    pub performances: Vec<PerformanceRef<'a>>,
    #[serde(borrow)]
    pub seat_category_names: BTreeMap<Cow<'a, str>, Cow<'a, str>>,
    #[serde(borrow)]
    pub topic_sub_topics: BTreeMap<Cow<'a, str>, Vec<u64>>,
    #[serde(borrow)]
    pub venue_names: BTreeMap<Cow<'a, str>, Cow<'a, str>>,
}

/// `citm-like`: ~480 KiB, integer- and key-heavy, many `null`s.
#[derive(Debug)]
pub struct CitmLike;

impl Workload for CitmLike {
    const NAME: &'static str = "citm-like";
    const GROUP: Group = Group::World;
    type Owned = Citm;
    type Borrowed<'de> = CitmRef<'de>;

    fn value() -> Citm {
        let mut rng = Rng::new(0xc17a);
        let names = |rng: &mut Rng, n: u64, base: u64| {
            (0..n)
                .map(|i| ((base + i).to_string(), sentence(rng, 2)))
                .collect::<BTreeMap<_, _>>()
        };
        let area_names = names(&mut rng, 17, 205_705_993);
        let seat_category_names = names(&mut rng, 64, 338_937_235);
        let venue_names = names(&mut rng, 1, 0);
        let events = (0..184u64)
            .map(|i| {
                let id = 138_586_341 + i * 7;
                (
                    id.to_string(),
                    Event {
                        description: None,
                        id,
                        logo: (rng.below(2) == 0).then(|| format!("/images/UE0AAAAACEKo{i}QAAAAVDSVRN")),
                        name: sentence(&mut rng, 3),
                        sub_topic_ids: (0..=rng.below(4)).map(|_| 337_184_262 + rng.below(100)).collect(),
                        subject_code: None,
                        subtitle: None,
                        topic_ids: (0..=rng.below(3)).map(|_| 324_846_099 + rng.below(100)).collect(),
                    },
                )
            })
            .collect();
        let performances = (0..243u64)
            .map(|i| Performance {
                event_id: 138_586_341 + (i % 184) * 7,
                id: 339_887_544 + i,
                logo: (rng.below(2) == 0).then(|| format!("/images/UE0AAAAACEKo{i}QAAAAVDSVRN")),
                name: None,
                prices: (0..=rng.below(8))
                    .map(|_| Price {
                        amount: 10_000 + rng.below(200_000),
                        audience_sub_category_id: 337_100_890,
                        seat_category_id: 338_937_235 + rng.below(64),
                    })
                    .collect(),
                seat_categories: (0..=rng.below(8))
                    .map(|_| SeatCategory {
                        areas: (0..=rng.below(12))
                            .map(|_| Area {
                                area_id: 205_705_993 + rng.below(17),
                                block_ids: Vec::new(),
                            })
                            .collect(),
                        seat_category_id: 338_937_235 + rng.below(64),
                    })
                    .collect(),
                seat_map_image: None,
                start: 1_372_701_600_000 + rng.below(100_000_000_000),
                venue_code: "PLEYEL_PLEYEL".into(),
            })
            .collect();
        let topic_sub_topics = (0..30u64)
            .map(|i| {
                let topics = (0..=rng.below(20)).map(|_| 337_184_262 + rng.below(100)).collect();
                ((324_846_099 + i).to_string(), topics)
            })
            .collect();
        Citm {
            area_names,
            events,
            performances,
            seat_category_names,
            topic_sub_topics,
            venue_names,
        }
    }
}

/// A polygon feature's geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Geometry {
    #[serde(rename = "type")]
    pub kind: String,
    pub coordinates: Vec<Vec<(f64, f64)>>,
}

/// A feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Feature {
    #[serde(rename = "type")]
    pub kind: String,
    pub properties: BTreeMap<String, String>,
    pub geometry: Geometry,
}

/// The feature collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct Canada {
    #[serde(rename = "type")]
    pub kind: String,
    pub features: Vec<Feature>,
}

/// `canada-like`: ~2.1 MiB, a polygon of ~56 000 coordinate pairs with full
/// precision floats.
#[derive(Debug)]
pub struct CanadaLike;

impl Workload for CanadaLike {
    const NAME: &'static str = "canada-like";
    const GROUP: Group = Group::World;
    type Owned = Canada;
    type Borrowed<'de> = Canada;

    fn value() -> Canada {
        let mut rng = Rng::new(0xca7ada);
        let ring = |rng: &mut Rng, n: u64| {
            (0..n)
                .map(|_| {
                    let lon = -141.0 + (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 88.0;
                    let lat = 41.0 + (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 42.0;
                    (lon, lat)
                })
                .collect::<Vec<_>>()
        };
        let coordinates = (0..480).map(|_| ring(&mut rng, 117)).collect();
        Canada {
            kind: "FeatureCollection".into(),
            features: vec![Feature {
                kind: "Feature".into(),
                properties: BTreeMap::from([("name".into(), "Canada".into())]),
                geometry: Geometry {
                    kind: "Polygon".into(),
                    coordinates,
                },
            }],
        }
    }
}
