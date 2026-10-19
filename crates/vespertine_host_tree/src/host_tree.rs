//! Retained host tree with atomic serialized mutation batches (APP-006) and compact
//! retained state (PRF-015).
//!
//! A [`HostTree`] holds retained nodes mutated by batches of operations arriving as
//! serialized JSON bytes. A batch is decoded directly from the bytes into operations,
//! every `setStyle` payload is resolved into a shared interned style, and only then is the
//! batch applied; a failure in either of the first two phases leaves the tree untouched.
//! Equivalent re-sent batches are observably idle: the redraw flag stays clear and no
//! revision changes.

mod decode;
pub mod instrument;
pub mod reference;
mod style;
mod tree;
pub mod workload;

pub use decode::MAX_ID;
pub use style::{
    SWEEP_FACTOR, SWEEP_FLOOR, SharedStyle, StyleTable, StyleTableStats, StyleValidator,
    content_eq, content_hash,
};
pub use tree::{
    BatchError, BatchErrorKind, BatchResult, Children, DocumentOrder, HostTree, NodeId, NodeRef,
    PreparedBatch, TreeOptions, TreeStats,
};
