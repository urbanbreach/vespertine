//! Benchmark and test support shared by the integration tests and the large-mount example:
//! a counting allocator, a deliberately naive reference build of the batch contract and the
//! PRF-015 workload generator. Included with `#[path]`; not part of the library API.

#![allow(dead_code)]

pub mod instrument;
pub mod reference;
pub mod workload;
