//! Allocation-free sorting of the per-kind primitive arrays of a scene (PRF-002).
//!
//! Each array is brought into nondecreasing `(order, [texture, tile,] original index)` order:
//!
//! 1. input that is already ordered is left untouched;
//! 2. otherwise, when the maximum order is at most [`COUNTING_SORT_FACTOR`] times the count,
//!    a stable counting placement computes an index permutation;
//! 3. otherwise compact `(order, index)` keys are sorted to obtain the permutation.
//!
//! The permutation is applied by gathering every record exactly once into a retained scratch
//! vector which then swaps places with the live vector. All index and key scratch vectors are
//! retained across frames.

use crate::{
    DrawOrder, MonochromeSprite, PaintSurface, Path, PolychromeSprite, ScaledPixels, Shadow,
    SubpixelSprite, Underline, scene::Quad,
};

/// Counting placement is used while `max_order <= COUNTING_SORT_FACTOR * count`.
pub(crate) const COUNTING_SORT_FACTOR: usize = 4;

/// A primitive record that can be sorted by draw order.
pub(crate) trait SortRecord {
    /// Whether records with equal order are further sorted by [`SortRecord::group_key`].
    const HAS_GROUP_KEY: bool = false;

    fn order(&self) -> DrawOrder;

    /// Texture identity and tile identity, used by sprites to group by texture and tile.
    fn group_key(&self) -> (u64, u32) {
        (0, 0)
    }
}

macro_rules! plain_record {
    ($($record:ty),* $(,)?) => {
        $(impl SortRecord for $record {
            #[inline]
            fn order(&self) -> DrawOrder {
                self.order
            }
        })*
    };
}

macro_rules! sprite_record {
    ($($record:ty),* $(,)?) => {
        $(impl SortRecord for $record {
            const HAS_GROUP_KEY: bool = true;

            #[inline]
            fn order(&self) -> DrawOrder {
                self.order
            }

            #[inline]
            fn group_key(&self) -> (u64, u32) {
                (
                    (u64::from(self.tile.texture_id.index) << 8)
                        | self.tile.texture_id.kind as u64,
                    self.tile.tile_id.0,
                )
            }
        })*
    };
}

plain_record!(Shadow, Quad, Path<ScaledPixels>, Underline, PaintSurface);
sprite_record!(MonochromeSprite, SubpixelSprite, PolychromeSprite);

/// Sort key of a sprite: order, texture identity, tile identity, original index.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SpriteKey {
    order: u32,
    texture: u64,
    tile: u32,
    index: u32,
}

/// Index-sized scratch shared by every kind.
#[derive(Default)]
pub(crate) struct IndexScratch {
    /// `source[destination]` is the original index of the record that moves to `destination`.
    source: Vec<u32>,
    counts: Vec<u32>,
    keys: Vec<u64>,
    sprite_keys: Vec<SpriteKey>,
    /// `(texture, tile, index)` keys of one order bucket of sprites.
    bucket_keys: Vec<u128>,
    /// Counting-sort counters and destination of one order bucket of sprites.
    tile_counts: Vec<u32>,
    bucket_out: Vec<u32>,
    /// `(tile << 32) | index` pairs of one order bucket and their radix ping-pong buffer.
    radix_pairs: Vec<u64>,
    radix_swap: Vec<u64>,
    /// Number of times any scratch vector had to grow.
    allocations: u64,
}

/// One retained gather vector per primitive kind.
#[derive(Default)]
pub(crate) struct GatherScratch {
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) subpixel_sprites: Vec<SubpixelSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
}

/// Retained scratch of a scene.
#[derive(Default)]
pub(crate) struct SortScratch {
    pub(crate) index: IndexScratch,
    pub(crate) gather: GatherScratch,
}

impl IndexScratch {
    /// Number of times sort scratch storage had to grow since the scene was created.
    pub(crate) fn allocations(&self) -> u64 {
        self.allocations
    }
}

fn reserve_counted<T>(vec: &mut Vec<T>, len: usize, allocations: &mut u64) {
    if vec.capacity() < len {
        vec.reserve(len - vec.len());
        *allocations += 1;
    }
}

#[inline]
fn sort_key<T: SortRecord>(record: &T) -> (DrawOrder, u64, u32) {
    let (texture, tile) = record.group_key();
    (record.order(), texture, tile)
}

/// Sorts `records` by `(order, group key, original index)`.
pub(crate) fn sort_records<T: SortRecord>(
    records: &mut Vec<T>,
    gather: &mut Vec<T>,
    index: &mut IndexScratch,
) {
    let len = records.len();
    if len < 2 || is_sorted::<T>(records) {
        return;
    }
    assert!(len <= u32::MAX as usize, "too many primitives to sort");

    let max_order = records
        .iter()
        .map(|record| record.order())
        .max()
        .unwrap_or(0) as usize;
    reserve_counted(&mut index.source, len, &mut index.allocations);
    if max_order <= COUNTING_SORT_FACTOR * len {
        counting_permutation(records, index, max_order);
    } else {
        key_permutation(records, index);
    }

    reserve_counted(gather, len, &mut index.allocations);
    apply_permutation(records, gather, &index.source);
}

fn is_sorted<T: SortRecord>(records: &[T]) -> bool {
    records
        .windows(2)
        .all(|pair| sort_key(&pair[0]) <= sort_key(&pair[1]))
}

fn counting_permutation<T: SortRecord>(
    records: &[T],
    scratch: &mut IndexScratch,
    max_order: usize,
) {
    let len = records.len();
    reserve_counted(&mut scratch.counts, max_order + 2, &mut scratch.allocations);
    scratch.counts.clear();
    scratch.counts.resize(max_order + 2, 0);
    for record in records {
        scratch.counts[record.order() as usize + 1] += 1;
    }
    for order in 1..scratch.counts.len() {
        scratch.counts[order] += scratch.counts[order - 1];
    }
    // `counts[o]` is now the first destination of order `o`.
    scratch.source.clear();
    scratch.source.resize(len, 0);
    for (index, record) in records.iter().enumerate() {
        let slot = &mut scratch.counts[record.order() as usize];
        scratch.source[*slot as usize] = index as u32;
        *slot += 1;
    }
    if T::HAS_GROUP_KEY {
        // `counts[o]` is now the end of bucket `o`; order each bucket by group key and index.
        let mut start = 0usize;
        for order in 0..=max_order {
            let end = scratch.counts[order] as usize;
            if end - start > 1 {
                sort_bucket(records, scratch, start, end);
            }
            start = end;
        }
    }
}

/// Orders `scratch.source[start..end]` (record indices of one order, ascending) by group key,
/// keeping the index order among equal keys.
///
/// The sprites of one order mostly share a texture and their tiles are small dense numbers, so
/// they are placed by a stable counting sort on the tile; anything else is sorted through packed
/// keys that are compared without touching the records.
fn sort_bucket<T: SortRecord>(records: &[T], scratch: &mut IndexScratch, start: usize, end: usize) {
    let len = end - start;
    let (first_texture, _) = records[scratch.source[start] as usize].group_key();
    let (mut one_texture, mut sorted) = (true, true);
    let (mut min_tile, mut max_tile) = (u32::MAX, 0u32);
    let mut last = (0u64, 0u32);
    for &index in &scratch.source[start..end] {
        let key = records[index as usize].group_key();
        one_texture &= key.0 == first_texture;
        sorted &= key >= last;
        last = key;
        min_tile = min_tile.min(key.1);
        max_tile = max_tile.max(key.1);
    }
    if sorted {
        return;
    }

    let range = (max_tile - min_tile) as usize;
    if one_texture && range <= COUNTING_SORT_FACTOR * len + 64 {
        reserve_counted(
            &mut scratch.tile_counts,
            range + 2,
            &mut scratch.allocations,
        );
        scratch.tile_counts.clear();
        scratch.tile_counts.resize(range + 2, 0);
        for &index in &scratch.source[start..end] {
            let tile = records[index as usize].group_key().1;
            scratch.tile_counts[(tile - min_tile) as usize + 1] += 1;
        }
        for tile in 1..scratch.tile_counts.len() {
            scratch.tile_counts[tile] += scratch.tile_counts[tile - 1];
        }
        reserve_counted(&mut scratch.bucket_out, len, &mut scratch.allocations);
        scratch.bucket_out.clear();
        scratch.bucket_out.resize(len, 0);
        for &index in &scratch.source[start..end] {
            let tile = records[index as usize].group_key().1;
            let slot = &mut scratch.tile_counts[(tile - min_tile) as usize];
            scratch.bucket_out[*slot as usize] = index;
            *slot += 1;
        }
        scratch.source[start..end].copy_from_slice(&scratch.bucket_out);
        return;
    }
    if one_texture {
        radix_sort_by_tile(records, scratch, start, end);
        return;
    }

    reserve_counted(&mut scratch.bucket_keys, len, &mut scratch.allocations);
    scratch.bucket_keys.clear();
    for &index in &scratch.source[start..end] {
        let (texture, tile) = records[index as usize].group_key();
        scratch
            .bucket_keys
            .push((u128::from(texture) << 64) | (u128::from(tile) << 32) | u128::from(index));
    }
    scratch.bucket_keys.sort_unstable();
    for (slot, key) in scratch.source[start..end]
        .iter_mut()
        .zip(&scratch.bucket_keys)
    {
        *slot = *key as u32;
    }
}

/// Buckets shorter than this are sorted by comparison instead of by digit.
const RADIX_MIN_LEN: usize = 256;

/// Orders `scratch.source[start..end]` (record indices of one order and one texture, ascending)
/// by tile identity: a stable least-significant-digit radix sort over the tile's bytes, skipping
/// the byte positions at which every record has the same digit, for large buckets and a sort of
/// packed `(tile, index)` keys for small ones.
fn radix_sort_by_tile<T: SortRecord>(
    records: &[T],
    scratch: &mut IndexScratch,
    start: usize,
    end: usize,
) {
    let len = end - start;
    reserve_counted(&mut scratch.radix_pairs, len, &mut scratch.allocations);
    scratch.radix_pairs.clear();
    for &index in &scratch.source[start..end] {
        let tile = records[index as usize].group_key().1;
        scratch
            .radix_pairs
            .push((u64::from(tile) << 32) | u64::from(index));
    }
    if len < RADIX_MIN_LEN {
        // The index in the low half makes every key unique, so the unstable sort is stable.
        scratch.radix_pairs.sort_unstable();
        for (slot, pair) in scratch.source[start..end]
            .iter_mut()
            .zip(&scratch.radix_pairs)
        {
            *slot = *pair as u32;
        }
        return;
    }

    reserve_counted(&mut scratch.radix_swap, len, &mut scratch.allocations);
    scratch.radix_swap.clear();
    scratch.radix_swap.resize(len, 0);
    let mut histograms = [[0u32; 256]; 4];
    for &pair in &scratch.radix_pairs {
        for (byte, histogram) in histograms.iter_mut().enumerate() {
            histogram[(pair >> (32 + 8 * byte)) as usize & 0xff] += 1;
        }
    }

    let mut from = &mut scratch.radix_pairs;
    let mut to = &mut scratch.radix_swap;
    for (byte, histogram) in histograms.iter_mut().enumerate() {
        let shift = 32 + 8 * byte;
        let first_digit = (from[0] >> shift) as usize & 0xff;
        if histogram[first_digit] as usize == len {
            continue;
        }
        let mut offset = 0u32;
        for count in histogram.iter_mut() {
            let next = offset + *count;
            *count = offset;
            offset = next;
        }
        for &pair in from.iter() {
            let slot = &mut histogram[(pair >> shift) as usize & 0xff];
            to[*slot as usize] = pair;
            *slot += 1;
        }
        std::mem::swap(&mut from, &mut to);
    }
    for (slot, pair) in scratch.source[start..end].iter_mut().zip(from.iter()) {
        *slot = *pair as u32;
    }
}

fn key_permutation<T: SortRecord>(records: &[T], scratch: &mut IndexScratch) {
    let len = records.len();
    scratch.source.clear();
    if T::HAS_GROUP_KEY {
        reserve_counted(&mut scratch.sprite_keys, len, &mut scratch.allocations);
        scratch.sprite_keys.clear();
        scratch
            .sprite_keys
            .extend(records.iter().enumerate().map(|(index, record)| {
                let (texture, tile) = record.group_key();
                SpriteKey {
                    order: record.order(),
                    texture,
                    tile,
                    index: index as u32,
                }
            }));
        scratch.sprite_keys.sort_unstable();
        scratch
            .source
            .extend(scratch.sprite_keys.iter().map(|key| key.index));
    } else {
        reserve_counted(&mut scratch.keys, len, &mut scratch.allocations);
        scratch.keys.clear();
        scratch.keys.extend(
            records
                .iter()
                .enumerate()
                .map(|(index, record)| (u64::from(record.order()) << 32) | index as u64),
        );
        scratch.keys.sort_unstable();
        scratch
            .source
            .extend(scratch.keys.iter().map(|key| *key as u32));
    }
}

/// Moves `records[source[i]]` to position `i` for every `i`, touching each record once.
/// `source` must be a permutation of `0..records.len()`.
fn apply_permutation<T>(records: &mut Vec<T>, gather: &mut Vec<T>, source: &[u32]) {
    let len = records.len();
    assert_eq!(source.len(), len);
    gather.clear();
    gather.reserve(len);
    // SAFETY: `source` is a permutation of `0..len` built by this module, so every record is
    // moved out of `records` exactly once. `records` is set to length 0 before the buffers
    // are exchanged so that moved-out records are never dropped there, and `gather` is given
    // length `len` only after all `len` slots were written. Indexing is bounds-checked and
    // nothing in the loop can unwind.
    unsafe {
        let from = records.as_ptr();
        let to = gather.as_mut_ptr();
        for (destination, &index) in source.iter().enumerate() {
            assert!((index as usize) < len);
            std::ptr::copy_nonoverlapping(from.add(index as usize), to.add(destination), 1);
        }
        records.set_len(0);
        gather.set_len(len);
    }
    std::mem::swap(records, gather);
}

#[cfg(test)]
mod tests;
