//! Phase 1 of a batch (APP-006 R1-R4, PRF-015 R2): decoding the transport bytes directly
//! into operations. No general-purpose value tree is built for the batch. Style payloads
//! stay as borrowed byte slices of the transport text until they are interned; only custom
//! property values (small, per operation) are parsed into values.

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, IgnoredAny, SeqAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::style::SharedStyle;

/// Largest valid element identifier: 2^53 - 1.
pub const MAX_ID: u64 = 9_007_199_254_740_991;

/// A style operand: raw transport bytes after phase 1, the shared style after phase 2.
pub(crate) enum StyleSource<'a> {
    Raw(&'a RawValue),
    Resolved(SharedStyle),
}

pub(crate) enum Op<'a> {
    Create {
        id: u64,
        element_type: Cow<'a, str>,
    },
    Destroy(u64),
    Append {
        child: u64,
        parent: u64,
    },
    InsertBefore {
        before: u64,
        parent: u64,
        child: u64,
    },
    SetStyle {
        id: u64,
        style: StyleSource<'a>,
    },
    SetText {
        id: u64,
        text: Cow<'a, str>,
    },
    SetEventListener {
        id: u64,
        event: Cow<'a, str>,
        has_handler: bool,
    },
    SetRoot(u64),
    SetKeyboardFocusDim(bool),
    SetCustomProp {
        id: u64,
        key: Cow<'a, str>,
        value: Box<Value>,
    },
}

/// The first failure found while decoding, recorded beside the serde error so the message
/// can name the failing operation.
#[derive(Default)]
pub(crate) struct Failure {
    pub index: usize,
    pub message: String,
}

pub(crate) fn decode_batch<'a>(
    input: &'a str,
) -> Result<Vec<Op<'a>>, (Option<Failure>, serde_json::Error)> {
    let failure = RefCell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let result = BatchSeed { failure: &failure }.deserialize(&mut deserializer);
    let result = result.and_then(|ops| deserializer.end().map(|()| ops));
    result.map_err(|error| (failure.into_inner(), error))
}

struct BatchSeed<'f> {
    failure: &'f RefCell<Option<Failure>>,
}

impl<'de> DeserializeSeed<'de> for BatchSeed<'_> {
    type Value = Vec<Op<'de>>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for BatchSeed<'_> {
    type Value = Vec<Op<'de>>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON array of operation tuples")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut ops = Vec::new();
        let mut index = 0usize;
        loop {
            let slot = OpSeed {
                index,
                failure: self.failure,
            };
            match seq.next_element_seed(slot) {
                Ok(Some(op)) => ops.push(op),
                Ok(None) => return Ok(ops),
                Err(error) => {
                    let mut failure = self.failure.borrow_mut();
                    if failure.is_none() {
                        *failure = Some(Failure {
                            index,
                            message: error.to_string(),
                        });
                    }
                    return Err(error);
                }
            }
            index += 1;
        }
    }
}

struct OpSeed<'f> {
    index: usize,
    failure: &'f RefCell<Option<Failure>>,
}

impl OpSeed<'_> {
    fn fail<E: de::Error>(&self, message: impl fmt::Display) -> E {
        let message = message.to_string();
        *self.failure.borrow_mut() = Some(Failure {
            index: self.index,
            message: message.clone(),
        });
        E::custom(message)
    }
}

impl<'de> DeserializeSeed<'de> for OpSeed<'_> {
    type Value = Op<'de>;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Op<'de>, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for OpSeed<'_> {
    type Value = Op<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an operation tuple (a JSON array whose first element is the operation name)")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Op<'de>, A::Error> {
        let name: Cow<'de, str> = match seq.next_element_seed(StrSeed("operation name"))? {
            Some(name) => name,
            None => return Err(self.fail("operation tuple is empty (missing operation name)")),
        };

        macro_rules! arg {
            ($seed:expr, $what:expr) => {
                match seq.next_element_seed($seed)? {
                    Some(value) => value,
                    None => {
                        return Err(
                            self.fail(format_args!("{} is missing argument {}", name, $what))
                        );
                    }
                }
            };
        }

        let op = match &*name {
            "createElement" => Op::Create {
                id: arg!(IdSeed, "id"),
                element_type: arg!(StrSeed("element type string"), "elementType"),
            },
            "destroyElement" => Op::Destroy(arg!(IdSeed, "id")),
            "appendChild" => Op::Append {
                parent: arg!(IdSeed, "parentId"),
                child: arg!(IdSeed, "childId"),
            },
            "insertBefore" => Op::InsertBefore {
                parent: arg!(IdSeed, "parentId"),
                child: arg!(IdSeed, "childId"),
                before: arg!(IdSeed, "beforeId"),
            },
            "setStyle" => Op::SetStyle {
                id: arg!(IdSeed, "id"),
                style: StyleSource::Raw(arg!(RawSeed(PhantomData), "style")),
            },
            "setText" => Op::SetText {
                id: arg!(IdSeed, "id"),
                text: arg!(StrSeed("text string"), "text"),
            },
            "setEventListener" => Op::SetEventListener {
                id: arg!(IdSeed, "id"),
                event: arg!(StrSeed("event type string"), "eventType"),
                has_handler: arg!(BoolSeed, "hasHandler"),
            },
            "setRoot" => Op::SetRoot(arg!(IdSeed, "id")),
            "setKeyboardFocusDim" => Op::SetKeyboardFocusDim(arg!(BoolSeed, "enabled")),
            "setCustomProp" => Op::SetCustomProp {
                id: arg!(IdSeed, "id"),
                key: arg!(StrSeed("property key string"), "key"),
                value: arg!(ValueSeed, "value"),
            },
            other => return Err(self.fail(format_args!("unknown operation {other:?}"))),
        };

        // Trailing arguments beyond the operation's own are ignored (APP-006 R4).
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(op)
    }
}

struct IdSeed;

impl<'de> DeserializeSeed<'de> for IdSeed {
    type Value = u64;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<u64, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl Visitor<'_> for IdSeed {
    type Value = u64;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JavaScript-safe nonnegative integer identifier (0 to 9007199254740991)")
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<u64, E> {
        if value <= MAX_ID {
            Ok(value)
        } else {
            Err(E::custom(format_args!(
                "identifier {value} exceeds 9007199254740991"
            )))
        }
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<u64, E> {
        if value < 0 {
            Err(E::custom(format_args!("identifier {value} is negative")))
        } else {
            self.visit_u64(value as u64)
        }
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<u64, E> {
        if !value.is_finite() || value.fract() != 0.0 {
            return Err(E::custom(format_args!(
                "identifier {value} is not an integral finite number"
            )));
        }
        if value < 0.0 {
            return Err(E::custom(format_args!("identifier {value} is negative")));
        }
        if value > MAX_ID as f64 {
            return Err(E::custom(format_args!(
                "identifier {value} exceeds 9007199254740991"
            )));
        }
        Ok(value as u64)
    }
}

struct BoolSeed;

impl<'de> DeserializeSeed<'de> for BoolSeed {
    type Value = bool;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<bool, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl Visitor<'_> for BoolSeed {
    type Value = bool;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON boolean")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<bool, E> {
        Ok(value)
    }
}

struct StrSeed(&'static str);

impl<'de> DeserializeSeed<'de> for StrSeed {
    type Value = Cow<'de, str>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Cow<'de, str>, D::Error> {
        deserializer.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for StrSeed {
    type Value = Cow<'de, str>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a {}", self.0)
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Cow<'de, str>, E> {
        Ok(Cow::Borrowed(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Cow<'de, str>, E> {
        Ok(Cow::Owned(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Cow<'de, str>, E> {
        Ok(Cow::Owned(value))
    }
}

struct RawSeed<'de>(PhantomData<&'de ()>);

impl<'de> DeserializeSeed<'de> for RawSeed<'de> {
    type Value = &'de RawValue;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<&'de RawValue, D::Error> {
        <&'de RawValue>::deserialize(deserializer)
    }
}

struct ValueSeed;

impl<'de> DeserializeSeed<'de> for ValueSeed {
    type Value = Box<Value>;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Box<Value>, D::Error> {
        Value::deserialize(deserializer).map(Box::new)
    }
}
