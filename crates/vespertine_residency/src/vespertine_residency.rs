//! Incremental document and media residency budgets (PRF-017).
//!
//! * [`block_document`]: block documents updated incrementally with stable block identities and
//!   anchors.
//! * [`fade`]: paint-only fade spans for newly arrived text.
//! * [`bounded_cache`]: bounded least-recently-used caches for derived results, with
//!   dependency keys.
//! * [`media_residency`] and [`image_residency`]: residency budgets for media that exist only
//!   while their content is expanded or visible, with explicit atlas release.
//! * [`identity`]: identity tokens that let asynchronous results verify their target before
//!   publishing.

pub mod block_document;
pub mod bounded_cache;
pub mod fade;
pub mod identity;
pub mod image_residency;
pub mod media_residency;

pub use block_document::{
    BlockAnchor, BlockDocument, BlockEntry, BlockId, BlockModel, BlockSelection, DocumentStats,
    ParsedBlock, UpdateStats,
};
pub use bounded_cache::{
    BoundedCache, CacheLimits, CacheStats, DependencyKey, DependencyKeyBuilder, Inserted, ThemeId,
};
pub use fade::{DocumentFades, FadeSpan, FadeSpans};
pub use identity::{IdentitySource, TargetIdentity, ThemeIdentity, Ticket};
pub use image_residency::{ImageResidency, decoded_bytes};
pub use media_residency::{
    ClassedResidency, Evicted, ResidencyConfig, ResidencyStats, ResidencyTracker,
};
