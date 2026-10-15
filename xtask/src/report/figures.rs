//! Pareto figures: deterministic SVG, so `report --check` can diff them.
//!
//! One scatter per workload, operation and trade-off: CPU (hardware cycles at
//! `native`, the axis the rule reads) against heap bytes, `.text` added, or
//! clean release compile time. Color carries a point's role, never its
//! identity, which its label carries: twelve entries are far more than a
//! scatter's colors can keep apart. Blue marks the frontier among every
//! measured entry; orange outlines the frontier among the entries rule 1
//! weighs (eligible, and not failing conformance); gray is everything else.

use std::fmt::Write as _;

use super::data::Data;
use super::verdict::{self, ENTRIES, EPSILON, Entry, X86_NATIVE, cpu, heap};
use crate::matrix::KYNOS;

/// The legend, as Markdown under the figures' heading.
pub const LEGEND: &str = "Each point is an entry point, labelled. The x-axis is hardware cycles at `native` (what rule 1 reads); the y-axis is the trade-off named in the title. Lower and further left is better on both. **Blue**, joined by a step line: the frontier among every measured entry (no other entry is better on one axis without being worse on the other, by more than ε = 5 % or, on cycles, the A/A band). **Orange ring and dashed line**: the frontier among the entries rule 1 weighs, the eligible ones that pass conformance. **Hollow**: the entry fails conformance. The gray crosshair is serde_json. Axes are logarithmic where the figures span more than 10×. Each SVG carries the figures as hover titles.";

/// One generated figure.
#[derive(Debug, Clone)]
pub struct Figure {
    pub group: &'static str,
    pub group_title: String,
    pub file: String,
    pub title: String,
    /// The entries on the all-entries frontier, by name.
    pub frontier: Vec<String>,
    pub svg: String,
}

/// A trade-off axis plotted against CPU.
#[derive(Clone, Copy)]
enum Trade {
    Heap,
    Text,
    Compile,
}

impl Trade {
    fn name(self) -> &'static str {
        match self {
            Self::Heap => "heap",
            Self::Text => "text",
            Self::Compile => "compile",
        }
    }

    fn label(self, op: &str) -> String {
        match self {
            Self::Heap => format!("{op} heap bytes per operation"),
            Self::Text => format!(
                "`.text` added, {} fixture (bytes)",
                if op == "encode" { "full" } else { "decode-only" }
            ),
            Self::Compile => "clean release build (s)".to_owned(),
        }
    }

    fn value(self, data: &Data, e: &Entry, workload: &str, op: &str) -> Option<f64> {
        match self {
            Self::Heap => heap(data, "native", e.backend, workload, op),
            Self::Text => verdict::text(data, e.set, "native", op == "encode"),
            Self::Compile => verdict::compile(data, e.set),
        }
    }
}

struct Pt {
    name: &'static str,
    x: f64,
    y: f64,
    conforms: bool,
    weighed: bool,
}

/// Every figure the report embeds, in report order.
pub fn figures(data: &Data, conforming: &[&Entry]) -> Vec<Figure> {
    let mut out = Vec::new();
    for w in KYNOS {
        for op in ["decode", "encode"] {
            for trade in [Trade::Heap, Trade::Text, Trade::Compile] {
                if let Some(f) = figure(data, conforming, "kynos", w, op, trade) {
                    out.push(f);
                }
            }
        }
    }
    for w in payloads::ALL
        .iter()
        .filter(|w| !KYNOS.contains(w) && !w.starts_with("sweep-"))
    {
        if let Some(f) = figure(data, conforming, "workloads", w, "decode", Trade::Heap) {
            out.push(f);
        }
    }
    out
}

/// The entries of `pts` (by index) no other entry dominates, on two axes
/// with tolerances `tx` and `ty`.
fn frontier(pts: &[&Pt], tx: f64, ty: f64) -> Vec<usize> {
    (0..pts.len())
        .filter(|&i| {
            let p = pts[i];
            !pts.iter().enumerate().any(|(j, q)| {
                j != i
                    && q.x <= p.x * (1.0 + tx)
                    && q.y <= p.y * (1.0 + ty)
                    && (q.x < p.x * (1.0 - tx) || q.y < p.y * (1.0 - ty))
            })
        })
        .collect()
}

fn figure(
    data: &Data,
    conforming: &[&Entry],
    group: &'static str,
    workload: &str,
    op: &str,
    trade: Trade,
) -> Option<Figure> {
    let entries: Vec<&Entry> = ENTRIES.iter().filter(|e| op == "decode" || e.encodes).collect();
    let pts: Vec<Pt> = entries
        .iter()
        .filter_map(|e| {
            let x = cpu(data, X86_NATIVE, e.backend, workload, op)?;
            let y = trade.value(data, e, workload, op)?;
            let conforms = conforming.iter().any(|c| c.backend == e.backend);
            Some(Pt {
                name: e.backend,
                x,
                y,
                conforms,
                weighed: conforms && e.eligible,
            })
        })
        .collect();
    if pts.len() < 2 {
        return None;
    }
    let tx = verdict::cpu_tolerance(data, X86_NATIVE, workload, op);
    let all: Vec<&Pt> = pts.iter().collect();
    let front = frontier(&all, tx, EPSILON);
    let weighed: Vec<&Pt> = pts.iter().filter(|p| p.weighed).collect();
    let weighed_front: Vec<&str> = frontier(&weighed, tx, EPSILON)
        .into_iter()
        .map(|i| weighed[i].name)
        .collect();
    let title = format!(
        "{workload} {op}: cycles at native against {}",
        trade.label(op).replace('`', "")
    );
    let file = format!("{workload}-{op}-{}.svg", trade.name());
    let base = pts.iter().find(|p| p.name == "serde_json").map(|p| (p.x, p.y));
    let svg = draw(&title, &trade.label(op).replace('`', ""), &pts, &front, &weighed_front, base);
    Some(Figure {
        group,
        group_title: if group == "kynos" {
            "The Kynos shapes".to_owned()
        } else {
            "Every other workload: decode cycles against heap".to_owned()
        },
        file,
        title,
        frontier: front.iter().map(|&i| format!("`{}`", pts[i].name)).collect(),
        svg,
    })
}

const W: f64 = 720.0;
const H: f64 = 440.0;
const LEFT: f64 = 84.0;
const RIGHT: f64 = 170.0;
const TOP: f64 = 48.0;
const BOTTOM: f64 = 56.0;

/// A scale from data to pixels, logarithmic when the data span 10× or more.
struct Scale {
    lo: f64,
    hi: f64,
    log: bool,
    p0: f64,
    p1: f64,
}

impl Scale {
    fn new(values: impl Iterator<Item = f64> + Clone, p0: f64, p1: f64) -> Self {
        let lo = values.clone().fold(f64::INFINITY, f64::min);
        let hi = values.fold(f64::NEG_INFINITY, f64::max);
        let log = lo > 0.0 && hi / lo >= 10.0;
        let (lo, hi) = if log {
            (lo / 1.25, hi * 1.25)
        } else {
            let pad = ((hi - lo) * 0.08).max(hi.abs() * 0.02).max(1e-9);
            ((lo - pad).max(0.0), hi + pad)
        };
        Self { lo, hi, log, p0, p1 }
    }

    fn at(&self, v: f64) -> f64 {
        let t = if self.log {
            (v.ln() - self.lo.ln()) / (self.hi.ln() - self.lo.ln())
        } else {
            (v - self.lo) / (self.hi - self.lo)
        };
        self.p0 + t * (self.p1 - self.p0)
    }

    fn ticks(&self) -> Vec<f64> {
        if self.log {
            let mut t = Vec::new();
            let mut decade = 10f64.powf(self.lo.log10().floor());
            while decade <= self.hi {
                for m in [1.0, 2.0, 5.0] {
                    let v = decade * m;
                    if v >= self.lo && v <= self.hi {
                        t.push(v);
                    }
                }
                decade *= 10.0;
            }
            t
        } else {
            let raw = (self.hi - self.lo) / 5.0;
            let mag = 10f64.powf(raw.log10().floor());
            let step = [1.0, 2.0, 2.5, 5.0, 10.0]
                .iter()
                .map(|m| m * mag)
                .find(|s| *s >= raw)
                .unwrap_or(raw);
            let mut v = (self.lo / step).ceil() * step;
            let mut t = Vec::new();
            while v <= self.hi + step * 1e-9 {
                t.push(v);
                v += step;
            }
            t
        }
    }
}

fn short(v: f64) -> String {
    match v.abs() {
        a if a >= 1e9 => format!("{:.3}G", v / 1e9),
        a if a >= 1e6 => format!("{:.3}M", v / 1e6),
        a if a >= 1e3 => format!("{:.3}k", v / 1e3),
        a if a >= 10.0 || a == 0.0 => format!("{v:.0}"),
        _ => format!("{v:.2}"),
    }
    .replace(".000", "")
    .trim_end_matches('0')
    .trim_end_matches('.')
    .to_owned()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

const STYLE: &str = "<style>
.bg{fill:#fcfcfb}.ink{fill:#0b0b0b}.ink2{fill:#52514e}.grid{stroke:#e4e3df;stroke-width:1}
.axis{stroke:#8a8984;stroke-width:1}.ref{stroke:#8a8984;stroke-width:1;stroke-dasharray:3 3}
.front{stroke:#2a78d6;stroke-width:2;fill:none}.weighed{stroke:#eb6834;stroke-width:2;fill:none;stroke-dasharray:6 4}
.on{fill:#2a78d6;stroke:#fcfcfb;stroke-width:2}.off{fill:#8a8984;stroke:#fcfcfb;stroke-width:2}
.on.hollow{fill:#fcfcfb;stroke:#2a78d6}.off.hollow{fill:#fcfcfb;stroke:#8a8984}
.ring{fill:none;stroke:#eb6834;stroke-width:2}
text{font-family:system-ui,-apple-system,'Segoe UI',sans-serif;font-size:12px}
.title{font-size:14px;font-weight:600}
@media (prefers-color-scheme:dark){
.bg{fill:#1a1a19}.ink{fill:#f0efec}.ink2{fill:#c3c2b7}.grid{stroke:#2e2e2c}.axis,.ref{stroke:#7a7974}
.front{stroke:#3987e5}.weighed{stroke:#d95926}.on{fill:#3987e5;stroke:#1a1a19}.off{fill:#7a7974;stroke:#1a1a19}
.on.hollow{fill:#1a1a19;stroke:#3987e5}.off.hollow{fill:#1a1a19;stroke:#7a7974}.ring{stroke:#d95926}}
</style>";

/// Where a label of `width` for the point at (`x`, `y`) goes: right of it,
/// at the nearest height (alternating up and down) clear of every label
/// already placed and inside the plot; left of it when the right has no
/// room. The label's baseline sits 4 px below its centre line.
fn place(placed: &[(f64, f64, f64)], x: f64, y: f64, width: f64) -> (f64, f64) {
    let clear = |lx: f64, ly: f64| {
        ly - 10.0 >= TOP
            && ly <= H - BOTTOM - 2.0
            && lx + width <= W - 4.0
            && lx >= LEFT + 2.0
            && !placed
                .iter()
                .any(|(px, py, pw)| (ly - py).abs() < 13.0 && lx < px + pw && *px < lx + width)
    };
    for side in [x + 12.0, x - 12.0 - width] {
        for step in 0..40 {
            let dy = f64::from((step + 1) / 2) * 13.0 * if step % 2 == 0 { 1.0 } else { -1.0 };
            let ly = y + 4.0 + dy;
            if clear(side, ly) {
                return (side, ly);
            }
        }
    }
    (x + 12.0, y + 4.0)
}

/// The title, grid, axes and serde_json's crosshair.
fn frame(s: &mut String, title: &str, ylabel: &str, sx: &Scale, sy: &Scale, base: Option<(f64, f64)>) {
    let _ = writeln!(s, "<text class=\"ink title\" x=\"{LEFT}\" y=\"24\">{}</text>", esc(title));
    for t in sx.ticks() {
        let x = sx.at(t);
        let _ = writeln!(
            s,
            "<line class=\"grid\" x1=\"{x:.1}\" y1=\"{TOP}\" x2=\"{x:.1}\" y2=\"{:.1}\"/><text class=\"ink2\" x=\"{x:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
            H - BOTTOM,
            H - BOTTOM + 18.0,
            short(t)
        );
    }
    for t in sy.ticks() {
        let y = sy.at(t);
        let _ = writeln!(
            s,
            "<line class=\"grid\" x1=\"{LEFT}\" y1=\"{y:.1}\" x2=\"{:.1}\" y2=\"{y:.1}\"/><text class=\"ink2\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
            W - RIGHT,
            LEFT - 8.0,
            y + 4.0,
            short(t)
        );
    }
    let _ = writeln!(
        s,
        "<line class=\"axis\" x1=\"{LEFT}\" y1=\"{b:.1}\" x2=\"{r:.1}\" y2=\"{b:.1}\"/><line class=\"axis\" x1=\"{LEFT}\" y1=\"{TOP}\" x2=\"{LEFT}\" y2=\"{b:.1}\"/>",
        b = H - BOTTOM,
        r = W - RIGHT
    );
    let _ = writeln!(
        s,
        "<text class=\"ink2\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">hardware cycles per operation, native{}</text>",
        (LEFT + W - RIGHT) / 2.0,
        H - 14.0,
        if sx.log { " (log)" } else { "" }
    );
    let _ = writeln!(
        s,
        "<text class=\"ink2\" transform=\"translate(18 {:.1}) rotate(-90)\" text-anchor=\"middle\">{}{}</text>",
        (TOP + H - BOTTOM) / 2.0,
        esc(ylabel),
        if sy.log { " (log)" } else { "" }
    );
    if let Some((bx, by)) = base {
        let _ = writeln!(
            s,
            "<line class=\"ref\" x1=\"{x:.1}\" y1=\"{TOP}\" x2=\"{x:.1}\" y2=\"{b:.1}\"/><line class=\"ref\" x1=\"{LEFT}\" y1=\"{y:.1}\" x2=\"{r:.1}\" y2=\"{y:.1}\"/>",
            x = sx.at(bx),
            y = sy.at(by),
            b = H - BOTTOM,
            r = W - RIGHT
        );
    }
}

/// A step line through `pts` sorted by x: across, then down.
fn steps(sx: &Scale, sy: &Scale, mut pts: Vec<(f64, f64)>) -> String {
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut d = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let (px, py) = (sx.at(*x), sy.at(*y));
        if i == 0 {
            let _ = write!(d, "M{px:.1},{py:.1}");
        } else {
            let _ = write!(d, " H{px:.1} V{py:.1}");
        }
    }
    d
}

fn draw(title: &str, ylabel: &str, pts: &[Pt], front: &[usize], weighed: &[&str], base: Option<(f64, f64)>) -> String {
    let sx = Scale::new(pts.iter().map(|p| p.x), LEFT, W - RIGHT);
    let sy = Scale::new(pts.iter().map(|p| p.y), H - BOTTOM, TOP);
    let mut s = String::new();
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {W} {H}\" width=\"{W}\" height=\"{H}\" role=\"img\" aria-label=\"{}\">\n{STYLE}\n<rect class=\"bg\" width=\"{W}\" height=\"{H}\" rx=\"6\"/>\n",
        esc(title)
    );
    frame(&mut s, title, ylabel, &sx, &sy, base);
    let front_pts: Vec<(f64, f64)> = front.iter().map(|&i| (pts[i].x, pts[i].y)).collect();
    if front_pts.len() > 1 {
        let _ = writeln!(s, "<path class=\"front\" d=\"{}\"/>", steps(&sx, &sy, front_pts));
    }
    let weighed_pts: Vec<(f64, f64)> = pts
        .iter()
        .filter(|p| weighed.contains(&p.name))
        .map(|p| (p.x, p.y))
        .collect();
    if weighed_pts.len() > 1 {
        let _ = writeln!(s, "<path class=\"weighed\" d=\"{}\"/>", steps(&sx, &sy, weighed_pts));
    }
    // Labels go right of their point and step down past any label already
    // placed there; a leader joins a displaced label to its point.
    let mut order: Vec<usize> = (0..pts.len()).collect();
    order.sort_by(|a, b| sy.at(pts[*a].y).total_cmp(&sy.at(pts[*b].y)));
    // Every point is an obstacle too: a box around the marker, in the same
    // (left, baseline, width) form as a label.
    let mut placed: Vec<(f64, f64, f64)> = pts
        .iter()
        .map(|p| (sx.at(p.x) - 7.0, sy.at(p.y) + 4.0, 14.0))
        .collect();
    for i in order {
        let p = &pts[i];
        let (x, y) = (sx.at(p.x), sy.at(p.y));
        let on = front.contains(&i);
        let class = format!(
            "{}{}",
            if on { "on" } else { "off" },
            if p.conforms { "" } else { " hollow" }
        );
        let _ = writeln!(
            s,
            "<circle class=\"{class}\" cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"5\"><title>{}: {} cycles, {}{}</title></circle>",
            esc(p.name),
            short(p.x),
            short(p.y),
            if p.conforms { "" } else { " (fails conformance)" }
        );
        if weighed.contains(&p.name) {
            let _ = writeln!(s, "<circle class=\"ring\" cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"9\"/>");
        }
        let (lx, ly) = place(&placed, x, y, p.name.len() as f64 * 6.6);
        placed.push((lx, ly, p.name.len() as f64 * 6.6));
        if (ly - (y + 4.0)).abs() > 1.0 || lx < x {
            let (from, to) = if lx < x { (x - 6.0, lx + p.name.len() as f64 * 6.6 + 2.0) } else { (x + 6.0, lx - 2.0) };
            let _ = writeln!(
                s,
                "<line class=\"ref\" x1=\"{from:.1}\" y1=\"{y:.1}\" x2=\"{to:.1}\" y2=\"{:.1}\"/>",
                ly - 4.0
            );
        }
        let _ = writeln!(s, "<text class=\"ink\" x=\"{lx:.1}\" y=\"{ly:.1}\">{}</text>", esc(p.name));
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::{Pt, frontier};

    fn pt(name: &'static str, x: f64, y: f64) -> Pt {
        Pt {
            name,
            x,
            y,
            conforms: true,
            weighed: true,
        }
    }

    #[test]
    fn the_frontier_keeps_trade_offs_and_drops_the_dominated() {
        let pts = [
            pt("fast-fat", 50.0, 200.0),
            pt("slow-lean", 100.0, 100.0),
            pt("worse", 110.0, 210.0),
            pt("tie", 102.0, 101.0),
        ];
        let refs: Vec<&Pt> = pts.iter().collect();
        let f: Vec<&str> = frontier(&refs, 0.05, 0.05).into_iter().map(|i| pts[i].name).collect();
        assert_eq!(f, ["fast-fat", "slow-lean", "tie"]);
    }

    /// Within the A/A band nothing is better, so a wide band keeps a point a
    /// narrow one drops.
    #[test]
    fn a_wide_band_keeps_what_noise_cannot_separate() {
        let pts = [pt("a", 100.0, 100.0), pt("b", 85.0, 100.0)];
        let refs: Vec<&Pt> = pts.iter().collect();
        assert_eq!(frontier(&refs, 0.05, 0.05), [1]);
        assert_eq!(frontier(&refs, 0.20, 0.05), [0, 1]);
    }
}
