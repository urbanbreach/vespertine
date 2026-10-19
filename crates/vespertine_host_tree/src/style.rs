//! Shared immutable styles and the interning table (PRF-015 R1-R3, R5; APP-006 R14).
//!
//! A style payload is opaque canonical JSON here; its schema belongs to the styling
//! specifications. The table keys payloads two ways:
//!
//! * by a hash of the trimmed raw payload bytes, so a payload seen before is resolved
//!   without parsing it again (a hit requires equal bytes), and
//! * by a hash of the parsed content that ignores object key order, so a payload that is
//!   equivalent to a known style (same properties and values, arrays in order) resolves to
//!   the same shared value and is parsed exactly once per distinct byte sequence.
//!
//! Hash collisions in either index are always resolved by comparing the actual bytes or
//! content, never by hash alone.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Weak};

use rustc_hash::FxHasher;
use serde_json::Value;

/// A parsed, immutable, shared style value.
pub type SharedStyle = Arc<Value>;

/// Smallest interned count that can trigger a sweep.
pub const SWEEP_FLOOR: usize = 64;
/// Growth/shrink factor of the sweep trigger.
pub const SWEEP_FACTOR: usize = 2;
/// Per-style cap on remembered distinct raw spellings (bounds memory for senders that
/// keep re-spelling one style; an uncached spelling is merely re-parsed).
const MAX_RAW_ALIASES: u8 = 8;

type FxBuild = BuildHasherDefault<FxHasher>;

/// Validates a parsed style payload against a style schema. The default only requires a
/// JSON object.
pub type StyleValidator = fn(&Value) -> Result<(), String>;

pub(crate) fn require_object(value: &Value) -> Result<(), String> {
    if value.is_object() {
        Ok(())
    } else {
        Err("style must be a JSON object".to_string())
    }
}

/// Structural equality by content: object key order is irrelevant, array order is
/// significant, numbers compare by numeric value.
pub fn content_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(x), Some(y)) = (x.as_i64(), y.as_i64()) {
                x == y
            } else if let (Some(x), Some(y)) = (x.as_u64(), y.as_u64()) {
                x == y
            } else {
                match (x.as_f64(), y.as_f64()) {
                    (Some(x), Some(y)) => x == y,
                    _ => false,
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| content_eq(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| content_eq(x, y)))
        }
        _ => false,
    }
}

/// Hash of a value consistent with [`content_eq`]: equal content hashes equally.
pub fn content_hash(value: &Value) -> u64 {
    fn mix(tag: u64, payload: u64) -> u64 {
        let mut hasher = FxHasher::default();
        hasher.write_u64(tag);
        hasher.write_u64(payload);
        hasher.finish()
    }
    match value {
        Value::Null => mix(1, 0),
        Value::Bool(b) => mix(2, *b as u64),
        Value::Number(n) => {
            // Integers and integral floats of equal value must hash alike.
            let float = n.as_f64().unwrap_or(0.0);
            let float = if float == 0.0 { 0.0 } else { float };
            mix(3, float.to_bits())
        }
        Value::String(s) => {
            let mut hasher = FxHasher::default();
            hasher.write(s.as_bytes());
            mix(4, hasher.finish())
        }
        Value::Array(items) => {
            let mut hasher = FxHasher::default();
            hasher.write_u64(items.len() as u64);
            for item in items {
                hasher.write_u64(content_hash(item));
            }
            mix(5, hasher.finish())
        }
        Value::Object(map) => {
            // Order independent: wrapping sum of per-member hashes.
            let mut sum = 0u64;
            for (key, value) in map {
                let mut hasher = FxHasher::default();
                hasher.write(key.as_bytes());
                hasher.write_u64(content_hash(value));
                sum = sum.wrapping_add(hasher.finish());
            }
            mix(6, sum ^ map.len() as u64)
        }
    }
}

struct Entry {
    style: SharedStyle,
    hash: u64,
    raw_aliases: u8,
}

/// Counters describing the table, for tests and instrumentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StyleTableStats {
    /// Entries currently interned.
    pub interned: usize,
    /// Entries remaining right after the previous sweep (0 before the first).
    pub after_last_sweep: usize,
    /// Sweeps performed so far.
    pub sweeps: u64,
    /// Distinct payload byte sequences parsed so far.
    pub parses: u64,
}

/// The interning table.
pub struct StyleTable {
    content: HashMap<u64, Vec<Entry>, FxBuild>,
    raw: HashMap<u64, Vec<(Box<[u8]>, Weak<Value>)>, FxBuild>,
    count: usize,
    after_last_sweep: usize,
    sweeps: u64,
    parses: u64,
    forced_hash: Option<u64>,
    validator: StyleValidator,
}

impl StyleTable {
    pub(crate) fn new(validator: StyleValidator, forced_hash: Option<u64>) -> Self {
        Self {
            content: HashMap::default(),
            raw: HashMap::default(),
            count: 0,
            after_last_sweep: 0,
            sweeps: 0,
            parses: 0,
            forced_hash,
            validator,
        }
    }

    pub fn stats(&self) -> StyleTableStats {
        StyleTableStats {
            interned: self.count,
            after_last_sweep: self.after_last_sweep,
            sweeps: self.sweeps,
            parses: self.parses,
        }
    }

    fn raw_hash(&self, bytes: &[u8]) -> u64 {
        if let Some(forced) = self.forced_hash {
            return forced;
        }
        let mut hasher = FxHasher::default();
        hasher.write(bytes);
        hasher.finish()
    }

    fn hash_of(&self, value: &Value) -> u64 {
        self.forced_hash.unwrap_or_else(|| content_hash(value))
    }

    /// Resolves a raw payload to its shared style, interning it if it is new.
    pub(crate) fn intern(&mut self, payload: &[u8]) -> Result<SharedStyle, String> {
        let bytes = payload.trim_ascii();
        let raw_hash = self.raw_hash(bytes);
        if let Some(bucket) = self.raw.get(&raw_hash) {
            for (known, weak) in bucket {
                if &**known == bytes {
                    if let Some(style) = weak.upgrade() {
                        return Ok(style);
                    }
                }
            }
        }

        let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        (self.validator)(&value)?;
        self.parses += 1;

        let hash = self.hash_of(&value);
        let bucket = self.content.entry(hash).or_default();
        let style = match bucket
            .iter_mut()
            .find(|entry| content_eq(&entry.style, &value))
        {
            Some(entry) => {
                if entry.raw_aliases >= MAX_RAW_ALIASES {
                    return Ok(entry.style.clone());
                }
                entry.raw_aliases += 1;
                entry.style.clone()
            }
            None => {
                let style = Arc::new(value);
                bucket.push(Entry {
                    style: style.clone(),
                    hash,
                    raw_aliases: 1,
                });
                self.count += 1;
                style
            }
        };
        self.raw
            .entry(raw_hash)
            .or_default()
            .push((bytes.into(), Arc::downgrade(&style)));
        Ok(style)
    }

    /// Whether the sweep trigger holds for `live_elements` retained nodes.
    pub(crate) fn sweep_due(&self, live_elements: usize) -> bool {
        self.count >= (SWEEP_FACTOR * self.after_last_sweep).max(SWEEP_FLOOR)
            || self.count > (SWEEP_FACTOR * live_elements).max(SWEEP_FLOOR)
    }

    /// Removes exactly the entries no node references and records the remaining count.
    pub(crate) fn sweep(&mut self) {
        self.content.retain(|_, bucket| {
            bucket.retain(|entry| Arc::strong_count(&entry.style) > 1);
            !bucket.is_empty()
        });
        self.raw.retain(|_, bucket| {
            bucket.retain(|(_, weak)| weak.strong_count() > 0);
            !bucket.is_empty()
        });
        self.count = self.content.values().map(Vec::len).sum();
        self.after_last_sweep = self.count;
        self.sweeps += 1;
    }

    /// Entry hash of an interned style (test support for collision tests).
    pub fn entry_hash(&self, style: &SharedStyle) -> Option<u64> {
        self.content
            .values()
            .flatten()
            .find(|entry| Arc::ptr_eq(&entry.style, style))
            .map(|entry| entry.hash)
    }
}
