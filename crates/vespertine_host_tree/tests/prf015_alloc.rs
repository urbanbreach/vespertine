//! PRF-015 A4: decoding a large batch of style-carrying operations does not scale with a
//! duplicate generic value tree, and builds the same tree as the reference build.

mod common;

use std::fmt::Write;

use vespertine_host_tree::HostTree;
use vespertine_host_tree::instrument::{self, AllocStats, CountingAllocator};
use vespertine_host_tree::reference::ReferenceTree;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const UNIQUE_PAYLOADS: usize = 16;

fn payload(k: usize, reverse: bool, pad: &str) -> String {
    let fields = [
        format!("\"id\":{k}"),
        format!("\"pad\":\"{pad}\""),
        "\"width\":\"100%\"".to_string(),
        "\"height\":16".to_string(),
    ];
    let ordered: Vec<&String> = if reverse {
        fields.iter().rev().collect()
    } else {
        fields.iter().collect()
    };
    format!(
        "{{{}}}",
        ordered
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// `hosts` hosts: container, rows with one of 8 styles in two key orders, text on every
/// 5th host. Returns the batch and its operation count.
fn style_batch(hosts: usize, pad_len: usize) -> (String, usize) {
    let pad = "z".repeat(pad_len);
    let mut out = String::from("[[\"createElement\",0,\"div\"],[\"setRoot\",0]");
    let mut ops = 2;
    for i in 1..hosts {
        let reverse = (i / 8) % 2 == 1;
        write!(
            out,
            ",[\"createElement\",{i},\"div\"],[\"appendChild\",0,{i}]"
        )
        .unwrap();
        write!(out, ",[\"setStyle\",{i},{}]", payload(i % 8, reverse, &pad)).unwrap();
        ops += 3;
        if i % 5 == 0 {
            write!(out, ",[\"setText\",{i},\"Row {i}\"]").unwrap();
            ops += 1;
        }
    }
    out.push(']');
    (out, ops)
}

fn measure_tree(batch: &str) -> (HostTree, AllocStats) {
    let mut tree = HostTree::new();
    instrument::reset();
    tree.apply_batch(batch).expect("batch applies");
    let stats = instrument::snapshot();
    (tree, stats)
}

fn measure_reference(batch: &str) -> (ReferenceTree, AllocStats) {
    let mut tree = ReferenceTree::new();
    instrument::reset();
    tree.apply_batch(batch).expect("batch applies");
    let stats = instrument::snapshot();
    (tree, stats)
}

#[test]
fn a4_decode_allocation_is_bounded_by_operations_plus_unique_styles() {
    let hosts = 4_000;
    let (batch, ops) = style_batch(hosts, 1_400);
    let (tree, stats) = measure_tree(&batch);
    println!(
        "tree: batch {} bytes, {ops} ops: {} allocations, {} bytes, peak {} bytes",
        batch.len(),
        stats.count,
        stats.bytes,
        stats.peak_bytes
    );
    let text_nodes = hosts / 5;
    assert!(
        stats.count as usize <= text_nodes + 100 * UNIQUE_PAYLOADS + 100,
        "allocation count {} exceeds operations plus unique styles",
        stats.count
    );
    // Without a duplicate value tree the transient peak is the retained nodes and the
    // operation list, far below the batch text itself.
    assert!(
        stats.peak_bytes < ops as i64 * 256,
        "peak {} bytes",
        stats.peak_bytes
    );
    assert!(stats.peak_bytes < batch.len() as i64 / 2);
    assert_eq!(tree.style_table_stats().interned, 8);
    assert_eq!(tree.style_table_stats().parses, UNIQUE_PAYLOADS as u64);

    // Reference: whole-batch generic value tree and a parsed style per node.
    let (reference, reference_stats) = measure_reference(&batch);
    println!(
        "reference: {} allocations, {} bytes, peak {} bytes",
        reference_stats.count, reference_stats.bytes, reference_stats.peak_bytes
    );
    assert!(reference_stats.peak_bytes > 4 * stats.peak_bytes);
    assert!(reference_stats.count > 20 * stats.count);
    // The resulting tree is identical.
    reference
        .check_equivalent(&tree)
        .expect("tree equals the reference build");
}

#[test]
fn a4_allocation_does_not_grow_with_style_size() {
    let hosts = 4_000;
    let (small_batch, ops) = style_batch(hosts, 1_400);
    let (large_batch, _) = style_batch(hosts, 14_000);
    let (_, small) = measure_tree(&small_batch);
    let (tree, large) = measure_tree(&large_batch);
    println!(
        "{ops} ops: small styles {} B / {} allocs, 10x larger styles {} B / {} allocs",
        small.bytes, small.count, large.bytes, large.count
    );
    // 4,000 ops carry 12.6 KB more style text each (50 MB of batch text); only the 16
    // distinct payloads are parsed, so the allocation difference stays small.
    assert!(large_batch.len() > small_batch.len() + 40_000_000);
    assert!(
        large.bytes < small.bytes + 2 * 1024 * 1024,
        "{} vs {}",
        large.bytes,
        small.bytes
    );
    assert_eq!(large.count, small.count);
    assert!(large.peak_bytes < small.peak_bytes + 2 * 1024 * 1024);
    let (reference, _) = measure_reference(&large_batch);
    reference
        .check_equivalent(&tree)
        .expect("tree equals the reference build");
}

#[test]
fn a4_resent_equal_styles_allocate_nothing_per_operation() {
    let hosts = 2_000;
    let (batch, _) = style_batch(hosts, 1_400);
    let (mut tree, _) = measure_tree(&batch);
    // Re-send every style in the opposite key order: all payloads were seen before? No:
    // the opposite order of each style is already known from the mount (both orders occur).
    let mut resend = String::from("[");
    for i in 1..hosts {
        if i > 1 {
            resend.push(',');
        }
        write!(
            resend,
            "[\"setStyle\",{i},{}]",
            payload(i % 8, (i / 8) % 2 == 0, &"z".repeat(1_400))
        )
        .unwrap();
    }
    resend.push(']');
    instrument::reset();
    let result = tree.apply_batch(&resend).unwrap();
    let stats = instrument::snapshot();
    assert!(!result.redraw);
    // Only the operation list and its growth allocate: no parsing, no per-node style.
    assert!(stats.count < 64, "{} allocations", stats.count);
    assert_eq!(tree.style_table_stats().parses, UNIQUE_PAYLOADS as u64);
}
