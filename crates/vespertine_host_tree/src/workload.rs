//! Generator of the `PERF-PRF-015` batches (PRF-015 performance workload).
//!
//! Host 0 is the column container, hosts 1 ..= hosts-1 are fixed-height rows. Row `i`
//! uses style `S_(i mod 8)`, serialized with the properties in forward key order when
//! `floor(i / 8)` is even and in reverse order when it is odd (equivalent but not
//! byte-identical). Every 5th host (`i mod 5 == 0`, host 0 included) has the text
//! `Row <i>`.
//!
//! Frame 0 mounts everything (100,000 create-and-insert operations, 100,000 set-style
//! operations and 20,000 set-text operations: 220,000 logical operations; on the wire a
//! create-and-insert is a `createElement` plus an `appendChild` tuple, and the container
//! additionally takes a `setRoot`). Frames 1..=100 re-send 1,000 styles each in the
//! opposite key order. Frames 101..=110 destroy the rows with `i mod 10 != 0` in a
//! 10,000-host range.

use std::fmt::Write;

/// Hosts of the full workload.
pub const FULL_HOSTS: u64 = 100_000;
/// Set-style operations per resend frame.
pub const RESEND_CHUNK: u64 = 1_000;
/// Host range destroyed per destroy frame.
pub const DESTROY_RANGE: u64 = 10_000;
pub const LAST_RESEND_FRAME: u64 = 100;
pub const FIRST_DESTROY_FRAME: u64 = 101;
pub const LAST_DESTROY_FRAME: u64 = 110;

/// The workload for a given host count.
pub struct Workload {
    hosts: u64,
    /// `styles[j][reverse]`: serialized `S_j` in forward or reverse key order.
    styles: Vec<[String; 2]>,
    container: String,
}

fn style_properties(j: u64) -> Vec<(String, String)> {
    let mut props: Vec<(String, String)> = Vec::new();
    let mut add = |key: &str, value: String| props.push((key.to_string(), value));
    add("width", "\"100%\"".into());
    add("height", "16".into());
    add("flexShrink", "0".into());
    add("background", format!("\"hsl({}, 55%, 65%)\"", 45 * j));
    let sides = ["Top", "Right", "Bottom", "Left"];
    for (k, side) in sides.iter().enumerate() {
        add(
            &format!("border{side}Width"),
            format!("{}", (j + k as u64) % 3),
        );
        add(&format!("padding{side}"), format!("{}", 2 + j + k as u64));
        add(
            &format!("margin{side}"),
            format!("{}", (j * 2 + k as u64) % 5),
        );
    }
    for (k, corner) in ["TopLeft", "TopRight", "BottomLeft", "BottomRight"]
        .iter()
        .enumerate()
    {
        add(
            &format!("border{corner}Radius"),
            format!("{}", 1 + (j + k as u64) % 6),
        );
    }
    add("borderColor", format!("\"hsl({}, 40%, 35%)\"", 45 * j + 20));
    add("borderStyle", "\"solid\"".into());
    add("gap", format!("{}", 4 + j));
    add("opacity", format!("{}", 0.5 + (j as f64) * 0.05));
    add(
        "fontFamily",
        "\"Noto Sans, Inter, Helvetica Neue, Arial, sans-serif\"".into(),
    );
    add("fontSize", "12".into());
    add("fontWeight", "400".into());
    add("lineHeight", format!("{}", 14 + j));
    add("color", "\"#1f2328\"".into());
    add("textAlign", "\"left\"".into());
    add("whiteSpace", "\"nowrap\"".into());
    add("overflow", "\"hidden\"".into());
    add("cursor", "\"default\"".into());
    add("pointerEvents", "\"auto\"".into());
    add("userSelect", "\"text\"".into());
    add(
        "boxShadow",
        format!(
            "[{{\"offsetX\":0,\"offsetY\":{j},\"blurRadius\":{b},\"spreadRadius\":0,\"color\":\"rgba(0,0,0,0.{c})\"}},\
             {{\"offsetX\":1,\"offsetY\":1,\"blurRadius\":2,\"spreadRadius\":-1,\"color\":\"#00000033\",\"inset\":true}},\
             {{\"offsetX\":0,\"offsetY\":{j},\"blurRadius\":{b2},\"spreadRadius\":1,\"color\":\"rgba(10,20,30,0.{c})\"}}]",
            b = 2 + j,
            b2 = 8 + j,
            c = 1 + j
        ),
    );
    add(
        "hover",
        format!(
            "{{\"backgroundColor\":\"hsl({}, 60%, 55%)\",\"opacity\":1,\"borderColor\":\"#336699\",\"cursor\":\"pointer\"}}",
            45 * j + 10
        ),
    );
    add(
        "active",
        format!(
            "{{\"backgroundColor\":\"hsl({}, 65%, 50%)\",\"opacity\":0.9,\"borderColor\":\"#224466\"}}",
            45 * j + 15
        ),
    );
    add(
        "selectionColor",
        format!("\"hsla({}, 90%, 50%, 0.35)\"", 45 * j),
    );
    add(
        "focusVisible",
        format!(
            "{{\"outlineWidth\":2,\"outlineColor\":\"hsl({}, 80%, 45%)\",\"outlineOffset\":1}}",
            45 * j
        ),
    );
    props
}

fn serialize(props: &[(String, String)], reverse: bool) -> String {
    let mut out = String::with_capacity(1600);
    out.push('{');
    let mut write_one = |first: &mut bool, (key, value): &(String, String)| {
        if !*first {
            out.push(',');
        }
        *first = false;
        let _ = write!(out, "\"{key}\":{value}");
    };
    let mut first = true;
    if reverse {
        props.iter().rev().for_each(|p| write_one(&mut first, p));
    } else {
        props.iter().for_each(|p| write_one(&mut first, p));
    }
    out.push('}');
    out
}

impl Workload {
    pub fn new(hosts: u64) -> Self {
        assert!(hosts >= 1, "the workload needs at least the container host");
        let styles = (0..8)
            .map(|j| {
                let props = style_properties(j);
                [serialize(&props, false), serialize(&props, true)]
            })
            .collect();
        let container = "{\"display\":\"flex\",\"flexDirection\":\"column\",\"width\":\"100%\",\
                         \"height\":\"100%\",\"overflow\":\"hidden\",\"backgroundColor\":\"#ffffff\"}"
            .to_string();
        Self {
            hosts,
            styles,
            container,
        }
    }

    pub fn hosts(&self) -> u64 {
        self.hosts
    }

    /// Serialized `S_j` in forward (`false`) or reverse (`true`) key order.
    pub fn style_payload(&self, j: u64, reverse: bool) -> &str {
        &self.styles[(j % 8) as usize][reverse as usize]
    }

    /// Whether the mount serialization of host `i` uses reverse key order.
    pub fn mount_reversed(i: u64) -> bool {
        (i / 8) % 2 == 1
    }

    /// Number of hosts with a text (`i mod 5 == 0`).
    pub fn text_hosts(&self) -> u64 {
        self.hosts.div_ceil(5)
    }

    /// Logical operation count of the mount frame: create-and-insert, set-style and
    /// set-text operations (220,000 for the full workload).
    pub fn mount_logical_ops(&self) -> u64 {
        2 * self.hosts + self.text_hosts()
    }

    /// Wire tuples of the mount frame (`createElement` + `appendChild` per row, `setRoot`
    /// for the container, plus the set-style and set-text tuples).
    pub fn mount_wire_ops(&self) -> u64 {
        3 * self.hosts + self.text_hosts()
    }

    /// The mount batch (frame 0).
    pub fn mount_batch(&self) -> String {
        let mut out = String::with_capacity(self.hosts as usize * 1500 + 1024);
        out.push('[');
        let mut first = true;
        let mut sep = |out: &mut String| {
            if !first {
                out.push(',');
            }
            first = false;
        };
        for i in 0..self.hosts {
            sep(&mut out);
            let _ = write!(out, "[\"createElement\",{i},\"div\"]");
            if i == 0 {
                out.push_str(",[\"setRoot\",0]");
            } else {
                let _ = write!(out, ",[\"appendChild\",0,{i}]");
            }
            let payload = if i == 0 {
                &self.container
            } else {
                self.style_payload(i % 8, Self::mount_reversed(i))
            };
            let _ = write!(out, ",[\"setStyle\",{i},{payload}]");
            if i % 5 == 0 {
                let _ = write!(out, ",[\"setText\",{i},\"Row {i}\"]");
            }
        }
        out.push(']');
        out
    }

    /// Frame `f` in 1 ..= 100: 1,000 set-style operations re-sending the style of hosts
    /// `1000 (f - 1) + 1 ..= 1000 f` (clamped to the existing rows) in the opposite key
    /// order from their mount.
    pub fn resend_batch(&self, f: u64) -> String {
        assert!((1..=LAST_RESEND_FRAME).contains(&f));
        let (low, high) = (
            RESEND_CHUNK * (f - 1) + 1,
            (RESEND_CHUNK * f).min(self.hosts - 1),
        );
        let mut out = String::with_capacity(RESEND_CHUNK as usize * 1500);
        out.push('[');
        for i in low..=high {
            if i != low {
                out.push(',');
            }
            let payload = self.style_payload(i % 8, !Self::mount_reversed(i));
            let _ = write!(out, "[\"setStyle\",{i},{payload}]");
        }
        out.push(']');
        out
    }

    /// Hosts destroyed by frame `f` in 101 ..= 110: `i mod 10 != 0` in
    /// `[10000 (f - 101) + 1, 10000 (f - 100)]`, clamped to the existing rows.
    pub fn destroy_targets(&self, f: u64) -> Vec<u64> {
        assert!((FIRST_DESTROY_FRAME..=LAST_DESTROY_FRAME).contains(&f));
        let (low, high) = (
            DESTROY_RANGE * (f - FIRST_DESTROY_FRAME) + 1,
            (DESTROY_RANGE * (f - FIRST_DESTROY_FRAME + 1)).min(self.hosts - 1),
        );
        (low..=high).filter(|i| i % 10 != 0).collect()
    }

    pub fn destroy_batch(&self, f: u64) -> String {
        let mut out = String::from("[");
        for (k, i) in self.destroy_targets(f).into_iter().enumerate() {
            if k != 0 {
                out.push(',');
            }
            let _ = write!(out, "[\"destroyElement\",{i}]");
        }
        out.push(']');
        out
    }

    /// The batch of frame `f` (0 mount, 1..=100 resend, 101..=110 destroy), or `None` for
    /// frames without mutations.
    pub fn frame_batch(&self, f: u64) -> Option<String> {
        match f {
            0 => Some(self.mount_batch()),
            1..=LAST_RESEND_FRAME => Some(self.resend_batch(f)),
            FIRST_DESTROY_FRAME..=LAST_DESTROY_FRAME => Some(self.destroy_batch(f)),
            _ => None,
        }
    }
}

/// What [`verify_against_reference`] observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    /// Window-invalidating commits among frames 1..=100 (must be 0).
    pub resend_redraw_commits: u64,
    /// Destroyed identities over frames 101..=110.
    pub destroyed: u64,
    pub final_live_elements: usize,
    pub final_interned: usize,
}

/// Runs the workload on a [`HostTree`](crate::HostTree) and on the straightforward
/// [`ReferenceTree`](crate::reference::ReferenceTree) in lockstep and checks that both
/// produce the identical tree after the mount, after the resend frames and after the
/// destroy frames, and identical destroyed identities for every destroy frame.
pub fn verify_against_reference(hosts: u64) -> Result<VerifyReport, String> {
    let workload = Workload::new(hosts);
    let mut tree = crate::HostTree::new();
    let mut reference = crate::reference::ReferenceTree::new();
    let mut report = VerifyReport {
        resend_redraw_commits: 0,
        destroyed: 0,
        final_live_elements: 0,
        final_interned: 0,
    };
    for frame in 0..=LAST_DESTROY_FRAME {
        let batch = workload.frame_batch(frame).expect("mutation frame");
        let applied = tree
            .apply_batch(&batch)
            .map_err(|error| format!("frame {frame}: {error}"))?;
        let expected = reference
            .apply_batch(&batch)
            .map_err(|error| format!("frame {frame} (reference): {error}"))?;
        if applied.destroyed != expected {
            return Err(format!("frame {frame}: destroyed identities differ"));
        }
        if (1..=LAST_RESEND_FRAME).contains(&frame) && applied.redraw {
            report.resend_redraw_commits += 1;
        }
        report.destroyed += applied.destroyed.len() as u64;
        if matches!(frame, 0 | LAST_RESEND_FRAME | LAST_DESTROY_FRAME) {
            reference
                .check_equivalent(&tree)
                .map_err(|error| format!("after frame {frame}: {error}"))?;
        }
    }
    report.final_live_elements = tree.len();
    report.final_interned = tree.style_table_stats().interned;
    Ok(report)
}
