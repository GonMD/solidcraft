//! Weldment members as cut from stock: each Structural Member body's profile, long-point length
//! and end cuts, recorded as the feature is evaluated, for the cut list.

use serde::{Deserialize, Serialize};

use crate::{FeatureKind, SketchId, WeldJoint};

/// A Structural Member's key for line `line` of group `group` (0: the feature's own `path`):
/// the line's id, or `N:line` for group number `N` (from 2).
pub fn member_key(group: usize, line: &str) -> String {
    if group == 0 { line.to_string() } else { format!("{}:{line}", group + 1) }
}

/// A Structural Member's members as skeleton lines: each one's sketch and line id, in key order.
pub fn member_lines(kind: &FeatureKind) -> Vec<(SketchId, &str)> {
    let FeatureKind::WeldMember { path_sketch, path, groups, .. } = kind else { return Vec::new() };
    let more = groups.iter().flat_map(|g| g.path.iter().map(|l| (g.path_sketch, l.as_str())));
    path.iter().map(|l| (*path_sketch, l.as_str())).chain(more).collect()
}

/// `old`'s joints given by the keys of the members on the same skeleton lines in `lines` (keyed
/// `keys`); a joint of a member no longer there is dropped.
pub fn rekey_joints(old: &FeatureKind, lines: &[(SketchId, &str)], keys: &[String]) -> Vec<WeldJoint> {
    let FeatureKind::WeldMember { joints, .. } = old else { return Vec::new() };
    let (was, old_keys) = (member_lines(old), old.weld_keys());
    let rekey = |key: &String| -> Option<String> {
        let line = was.get(old_keys.iter().position(|k| k == key)?)?;
        keys.get(lines.iter().position(|l| l == line)?).cloned()
    };
    joints
        .iter()
        .filter_map(|j| {
            let [a, b] = &j.members;
            let through = match &j.through {
                Some(t) => Some(rekey(t)?),
                None => None,
            };
            Some(WeldJoint { members: [rekey(a)?, rekey(b)?], corner: j.corner, through })
        })
        .collect()
}

/// How one end of a member is cut from the stock, as the saw sees it (angles in degrees, from
/// square).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cut", rename_all = "snake_case")]
pub enum MemberEnd {
    /// Square to the member.
    Square,
    /// One angle, about one of the profile's axes.
    Mitre { angle: f64 },
    /// Turned about both of the profile's axes: `mitre` about its y axis, `bevel` about its x axis.
    Compound { mitre: f64, bevel: f64 },
    /// Shaped to fit the outside of another member.
    Coped,
}

impl MemberEnd {
    /// The end's angles (mitre, bevel), zero where it has none.
    pub fn angles(&self) -> (f64, f64) {
        match *self {
            MemberEnd::Square | MemberEnd::Coped => (0.0, 0.0),
            MemberEnd::Mitre { angle } => (angle, 0.0),
            MemberEnd::Compound { mitre, bevel } => (mitre, bevel),
        }
    }

    /// Its name in a cut list: square, mitre, compound or coped.
    pub fn kind(&self) -> &'static str {
        match self {
            MemberEnd::Square => "square",
            MemberEnd::Mitre { .. } => "mitre",
            MemberEnd::Compound { .. } => "compound",
            MemberEnd::Coped => "coped",
        }
    }
}

/// One member as made by its Structural Member feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberCut {
    /// The member's body.
    pub body: String,
    /// The Structural Member feature that made it.
    pub feature: u64,
    /// Its skeleton line.
    pub line: String,
    /// The stock it is cut from: the profile sketch's name.
    pub profile: String,
    /// Overall length along the member (mm), end to end at the long points.
    pub length: f64,
    /// Its start and end cuts.
    pub ends: [MemberEnd; 2],
    /// Its skeleton line's start and end, in its feature's component frame.
    #[serde(default)]
    pub axis: [solidcraft_geom::Vec3; 2],
    /// Its profile's outline as offsets square to the line, for later members to butt against.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<solidcraft_geom::Vec3>,
    /// How each end's cut leans, in the profile's frame (see [`lean_of`]): with `symmetry`, what
    /// tells identical members from mirror images.
    #[serde(default)]
    pub leans: [Lean; 2],
    /// The profile's symmetries.
    #[serde(default)]
    pub symmetry: ProfileSymmetry,
    /// How its profile is mirrored from the stock's (`leans` are of the mirrored profile).
    #[serde(default, skip_serializing_if = "crate::WeldMirror::is_none")]
    pub mirror: crate::WeldMirror,
    /// Its start and end cut planes.
    #[serde(default)]
    pub cuts: [CutPlane; 2],
    /// What it is cut from, to make it again with other ends (Trim/Extend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock: Option<Stock>,
    /// The member each end is coped to fit (beyond its cut plane, which then runs through that
    /// member's line).
    #[serde(default)]
    pub copes: [Option<Cope>; 2],
    /// A bent member's centreline and bends (`None` for a straight one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bent: Option<Bent>,
    /// Its body as it was last made: a body that differs has been changed by a later feature.
    #[serde(skip)]
    pub made: MadeBody,
}

impl MemberCut {
    /// A point on the face of end `end` (0 its start, 1 its end), in the profile's material:
    /// where to pick that end.
    pub fn end_point(&self, end: usize) -> Option<solidcraft_geom::Vec3> {
        let stock = self.stock.as_ref()?;
        let cut = self.cuts.get(end)?;
        let frame = self.frame_at(end)?;
        let d = frame.normal();
        let p = frame.to_world(stock.region.interior_point());
        let along = cut.normal.dot(d);
        (along.abs() > 1e-9).then(|| p + d * (cut.normal.dot(cut.at - p) / along))
    }

    /// Which end (0 its start, 1 its end) has its cut plane within `tol` of `p`.
    pub fn end_at(&self, p: solidcraft_geom::Vec3, tol: f64) -> Option<usize> {
        let dist = |c: &CutPlane| c.normal.dot(p - c.at).abs();
        let (d0, d1) = (dist(&self.cuts[0]), dist(&self.cuts[1]));
        let end = usize::from(d1 < d0);
        (d0.min(d1) <= tol).then_some(end)
    }

    /// The profile's frame at end `end` (0 its start, 1 its end), its normal along the member.
    pub fn frame_at(&self, end: usize) -> Option<solidcraft_geom::Plane> {
        match (&self.bent, end) {
            (Some(b), 1) => Some(b.end_frame),
            _ => self.stock.as_ref().map(|s| s.frame),
        }
    }

    /// Whether it is a ring: a bent member whose centreline closes on itself.
    pub fn is_ring(&self) -> bool {
        self.bent.is_some() && self.axis[0].dist(self.axis[1]) < 1e-3
    }

    /// The length of stock to cut (mm): the member's length, plus the allowance at each end of a
    /// bent member, trimmed off after bending.
    pub fn cut_length(&self) -> f64 {
        self.length + self.bent.as_ref().map_or(0.0, |b| 2.0 * b.allowance)
    }

    /// Its volume (mm³) from its stock and cut planes, while `body` is still the shape made and
    /// neither end is coped: the profile's area times the length between the two planes through
    /// its centroid, exact for a prism cut by two planes, and no tessellation. `None` otherwise
    /// (and for a bent member).
    pub fn volume(&self, body: &crate::kernel::Body) -> Option<f64> {
        if !self.made.is_shape(body) || self.copes.iter().any(Option::is_some) || self.bent.is_some() {
            return None;
        }
        let stock = self.stock.as_ref()?;
        let d = stock.frame.normal();
        let c = stock.frame.to_world(stock.region.centroid());
        let at = |cut: &CutPlane| {
            let along = cut.normal.dot(d);
            (along.abs() > 1e-9).then(|| cut.normal.dot(cut.at - c) / along)
        };
        let v = stock.region.area() * (at(&self.cuts[1])? - at(&self.cuts[0])?).abs();
        (v.is_finite() && v > 0.0).then_some(v)
    }
}

/// What a weldment plate is made by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlateKind {
    EndCap,
    Gusset,
}

/// One plate (an End Cap's or a Gusset's) as made, for the cut list: its thickness and the
/// outline it is cut to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateCut {
    /// The plate's body.
    pub body: String,
    /// The feature that made it.
    pub feature: u64,
    pub kind: PlateKind,
    pub thickness: f64,
    /// The outline's extent in the plate's own frame, the longer side first (mm).
    pub size: [f64; 2],
    /// The outline's area (mm²) and its length round (mm).
    pub area: f64,
    pub perimeter: f64,
    /// Its body as made: a plate cut straight to its outline, `thickness` thick.
    #[serde(skip)]
    pub made: MadeBody,
}

impl PlateCut {
    /// A plate `thickness` thick cut to `outline`.
    pub fn new(body: String, feature: u64, kind: PlateKind, thickness: f64, outline: &solidcraft_geom::Loop2) -> PlateCut {
        let pts = outline.polyline(1e-3);
        let (mut lo, mut hi) =
            (solidcraft_geom::Vec2::new(f64::INFINITY, f64::INFINITY), solidcraft_geom::Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for p in &pts {
            lo = solidcraft_geom::Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = solidcraft_geom::Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let (w, h) = if pts.is_empty() { (0.0, 0.0) } else { (hi.x - lo.x, hi.y - lo.y) };
        let perimeter = outline.segs.iter().map(solidcraft_geom::Seg2::length).sum();
        PlateCut {
            body,
            feature,
            kind,
            thickness,
            size: [w.max(h), w.min(h)],
            area: outline.signed_area().abs(),
            perimeter,
            made: MadeBody::default(),
        }
    }

    /// Its volume (mm³), its outline's area times its thickness, while `body` is still the shape
    /// made (no tessellation); `None` otherwise.
    pub fn volume(&self, body: &crate::kernel::Body) -> Option<f64> {
        self.made.is_shape(body).then_some(self.area * self.thickness)
    }

    /// Whether `other` is the same plate: the same kind and thickness, and an outline of the
    /// same extent and area, within `tol` (mm).
    pub fn same(&self, other: &PlateCut, tol: f64) -> bool {
        self.kind == other.kind
            && (self.thickness - other.thickness).abs() <= tol
            && self.size.iter().zip(&other.size).all(|(a, b)| (a - b).abs() <= tol)
            && (self.perimeter - other.perimeter).abs() <= tol * 4.0
            && (self.area - other.area).abs() <= tol * self.perimeter.max(other.perimeter)
    }
}

/// Most seams one Weld Bead lists.
pub const MAX_SEAMS: usize = 256;

/// A Weld Bead as found, for the weld table and the view: the seams it welds between two bodies
/// and what it was asked to be. Cosmetic: no body holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bead {
    /// The feature, and its name (the bead's).
    pub feature: u64,
    pub name: String,
    /// The bodies it joins, by name.
    pub bodies: [String; 2],
    pub kind: crate::WeldBeadKind,
    /// A fillet's leg, or a groove weld's throat (mm).
    pub size: f64,
    /// Each seam welded, as a polyline along it.
    pub seams: Vec<Vec<solidcraft_geom::Vec3>>,
    /// Intermittent welds: each one's length and the pitch between their centres (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intermittent: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub field: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Bead {
    /// Each seam's length (mm).
    pub fn seam_lengths(&self) -> Vec<f64> {
        self.seams.iter().map(|s| s.windows(2).map(|w| if let [a, b] = w { a.dist(*b) } else { 0.0 }).sum()).collect()
    }

    /// The seams' length in all (mm).
    pub fn length(&self) -> f64 {
        self.seam_lengths().iter().sum()
    }

    /// The length welded (mm) and in how many welds: each seam end to end, or for intermittent
    /// welds as many as fit along it from one end, the last ending at most at the seam's end (a
    /// seam shorter than one weld is welded all along).
    pub fn welded(&self) -> (f64, usize) {
        let lengths = self.seam_lengths();
        match self.intermittent {
            None => (lengths.iter().sum(), lengths.len()),
            Some([l, p]) => lengths.iter().fold((0.0, 0), |(total, n), s| {
                if *s <= l || !(p > 0.0) {
                    (total + s, n + 1)
                } else {
                    let k = (((s - l) / p + 1e-9).floor() as usize).saturating_add(1);
                    (total + l * k as f64, n + k)
                }
            }),
        }
    }

    /// The weld metal's volume (mm³): a fillet's triangle of legs `size` along the length
    /// welded; `None` for a groove weld, whose cross-section depends on the joint's preparation.
    pub fn volume(&self) -> Option<f64> {
        self.kind.is_fillet().then(|| self.size * self.size / 2.0 * self.welded().0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CutPlane {
    pub at: solidcraft_geom::Vec3,
    pub normal: solidcraft_geom::Vec3,
}

/// The stock a member is cut from: its profile, and the profile's frame at the start of the
/// member's line (its normal along the line).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stock {
    pub region: solidcraft_geom::Region2,
    pub frame: solidcraft_geom::Plane,
}

/// The outside of a member that another member's end is coped to fit: its profile's outer
/// outline in `frame`, from `from` to `to` along the frame's normal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cope {
    pub region: solidcraft_geom::Region2,
    pub frame: solidcraft_geom::Plane,
    pub from: f64,
    pub to: f64,
}

/// A bent member: its centreline from its start, its bends and its profile's frame at its end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bent {
    pub path: Vec<PathPiece>,
    pub bends: Vec<Bend>,
    /// The centreline's length, start to end (mm).
    pub developed: f64,
    /// How far the stock reaches past each end of the centreline to its long point (mm, negative
    /// where it stops short).
    pub lead: [f64; 2],
    /// The profile's frame at the member's end (as [`Stock::frame`] at its start).
    pub end_frame: solidcraft_geom::Plane,
    /// Stock added at each end for the bender (a roller's straight tails, a bender's grip),
    /// trimmed off after bending (mm).
    #[serde(default)]
    pub allowance: f64,
    /// Whether part of its centreline follows a free-form curve (as short lines): it is formed to
    /// that shape, not bent at bends (its `bends` are only its arcs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub formed: bool,
}

/// One piece of a bent member's centreline: a straight run, or an arc turning by `angle` (radians,
/// right hand) about `axis` through `center` from `a`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "piece", rename_all = "snake_case")]
pub enum PathPiece {
    Line { a: solidcraft_geom::Vec3, b: solidcraft_geom::Vec3 },
    Arc { a: solidcraft_geom::Vec3, center: solidcraft_geom::Vec3, axis: solidcraft_geom::Vec3, angle: f64 },
}

/// One bend of a bent member, as a bender makes it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bend {
    /// Where it starts along the centreline from the centreline's start (mm).
    pub at: f64,
    /// How far it turns (degrees).
    pub angle: f64,
    /// Its centreline radius (mm).
    pub radius: f64,
    /// Toward its centre, in the profile's frame there (unit `[x, y]`): the plane it bends in.
    pub toward: [f64; 2],
}

impl Bend {
    /// Its centreline length (mm).
    pub fn length(&self) -> f64 {
        self.radius * self.angle.to_radians()
    }
}

/// One step of bending a member: feed `feed` mm of straight (from the member's long point at its
/// start, or from the last bend), turn the stock by `rotation` degrees since the last bend (0
/// for the first), then bend it by `angle` degrees at centreline radius `radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendStep {
    pub feed: f64,
    pub rotation: f64,
    pub angle: f64,
    pub radius: f64,
}

/// A bent member's bends as a bender's steps, and the straight left after the last bend (to the
/// long point at its end). Allowances are not included.
pub fn bend_steps(b: &Bent) -> (Vec<BendStep>, f64) {
    let mut steps = Vec::with_capacity(b.bends.len());
    let mut done = -b.lead[0];
    let mut last: Option<[f64; 2]> = None;
    for bend in &b.bends {
        let rotation = last.map_or(0.0, |[x, y]| {
            let [u, v] = bend.toward;
            (x * v - y * u).atan2(x * u + y * v).to_degrees()
        });
        steps.push(BendStep { feed: bend.at - done, rotation, angle: bend.angle, radius: bend.radius });
        done = bend.at + bend.length();
        last = Some(bend.toward);
    }
    (steps, b.developed + b.lead[1] - done)
}

/// `b` turned end for end: its bends from its other end, each in the profile's frame turned end
/// for end (its x axis reversed).
fn reversed(b: &Bent) -> Bent {
    let bends =
        b.bends.iter().rev().map(|q| Bend { at: (b.developed - q.at - q.length()).max(0.0), toward: [-q.toward[0], q.toward[1]], ..*q }).collect();
    Bent { bends, lead: [b.lead[1], b.lead[0]], ..b.clone() }
}

/// Whether two members are bent alike (two straight ones are): the same bends, in order or end
/// for end, each within `length_tol` mm and `angle_tol` degrees and turning the same way between
/// bends; for a profile that isn't round, also in the same planes of the profile.
pub fn same_bends(a: &MemberCut, b: &MemberCut, length_tol: f64, angle_tol: f64) -> bool {
    let (x, y) = match (&a.bent, &b.bent) {
        (None, None) => return true,
        (Some(x), Some(y)) if x.formed || y.formed => return x.formed && y.formed && same_shape(x, y, length_tol, angle_tol),
        (Some(x), Some(y)) if x.bends.len() == y.bends.len() => (x, y),
        _ => return false,
    };
    let round = a.symmetry.round && b.symmetry.round;
    let near = |p: f64, q: f64, tol: f64| (p - q).abs() <= tol;
    let turn_near = |p: f64, q: f64| ((p - q + 540.0).rem_euclid(360.0) - 180.0).abs() <= angle_tol;
    let (sx, tx) = bend_steps(x);
    let fits = |y: &Bent| {
        let (sy, ty) = bend_steps(y);
        sx.len() == sy.len()
            && near(tx, ty, length_tol)
            && sx.iter().zip(&sy).all(|(p, q)| {
                near(p.feed, q.feed, length_tol)
                    && near(p.angle, q.angle, angle_tol)
                    && near(p.radius, q.radius, length_tol)
                    && turn_near(p.rotation, q.rotation)
            })
            && (round || x.bends.iter().zip(&y.bends).all(|(p, q)| near(p.toward[0], q.toward[0], 1e-3) && near(p.toward[1], q.toward[1], 1e-3)))
    };
    fits(y) || fits(&reversed(y))
}

/// One step along a centreline's shape: a piece, or the turn from one piece into the next.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ShapeStep {
    /// A piece's length (mm) and how far it turns (degrees, 0 for a straight one).
    Piece(f64, f64),
    /// The turn between two pieces (degrees).
    Turn(f64),
}

/// A centreline's shape, whatever its place, in order.
fn shape(b: &Bent) -> Vec<ShapeStep> {
    use solidcraft_geom::Vec3;
    let mut out = Vec::with_capacity(b.path.len() * 2);
    let mut last: Option<Vec3> = None;
    for p in &b.path {
        let (d0, d1, length, turn) = match *p {
            PathPiece::Line { a, b } => {
                let Some(d) = (b - a).normalized() else { continue };
                (d, d, a.dist(b), 0.0)
            }
            PathPiece::Arc { a, center, axis, angle } => {
                let Some(k) = axis.normalized() else { continue };
                let u = a - center;
                let (s, c) = angle.sin_cos();
                let end = u * c + k.cross(u) * s + k * (k.dot(u) * (1.0 - c));
                let (Some(d0), Some(d1)) = (k.cross(u).normalized(), k.cross(end).normalized()) else { continue };
                (d0, d1, u.len() * angle, angle.to_degrees())
            }
        };
        if let Some(l) = last {
            out.push(ShapeStep::Turn(l.dot(d0).clamp(-1.0, 1.0).acos().to_degrees()));
        }
        out.push(ShapeStep::Piece(length, turn));
        last = Some(d1);
    }
    out
}

/// Whether two formed members' centrelines have one shape, in order or end for end, with the
/// same lengths past their ends: each piece as long and each turn as large (within `length_tol`
/// mm and `angle_tol` degrees). Which way each turn goes is not compared, nor how the profile
/// lies along it.
fn same_shape(x: &Bent, y: &Bent, length_tol: f64, angle_tol: f64) -> bool {
    let (sx, sy) = (shape(x), shape(y));
    if sx.len() != sy.len() {
        return false;
    }
    let near = |p: &ShapeStep, q: &ShapeStep| match (p, q) {
        (ShapeStep::Piece(l, t), ShapeStep::Piece(m, u)) => (l - m).abs() <= length_tol && (t - u).abs() <= angle_tol,
        (ShapeStep::Turn(t), ShapeStep::Turn(u)) => (t - u).abs() <= angle_tol,
        _ => false,
    };
    let leads = |a: f64, b: f64| (a - b).abs() <= length_tol;
    (leads(x.lead[0], y.lead[0]) && leads(x.lead[1], y.lead[1]) && sx.iter().zip(&sy).all(|(p, q)| near(p, q)))
        || (leads(x.lead[0], y.lead[1]) && leads(x.lead[1], y.lead[0]) && sx.iter().zip(sy.iter().rev()).all(|(p, q)| near(p, q)))
}

/// A member's body as last made. Not saved, and not part of a member's equality.
#[derive(Clone, Debug, Default)]
pub struct MadeBody(pub Option<crate::kernel::Body>);

impl PartialEq for MadeBody {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl MadeBody {
    /// Whether `body` is still the one made.
    pub fn is(&self, body: &crate::kernel::Body) -> bool {
        self.0.as_ref().is_some_and(|b| b.same(body))
    }

    /// Whether `body` still has the shape made (whatever its colour or paint since).
    pub fn is_shape(&self, body: &crate::kernel::Body) -> bool {
        self.0.as_ref().is_some_and(|b| b.same_shape(body))
    }
}

/// How an end's cut leans from square, seen from outside the member: its outward normal's parts
/// along the profile's x and y axes over its part along the member (`[0, 0]` for a square cut).
pub type Lean = [f64; 2];

/// The lean of a cut with normal `normal` (either way) at the start or end of a member running
/// along `dir`, whose profile's axes are `x` and `y`.
pub fn lean_of(normal: solidcraft_geom::Vec3, dir: solidcraft_geom::Vec3, x: solidcraft_geom::Vec3, y: solidcraft_geom::Vec3, start: bool) -> Lean {
    let along = normal.dot(dir);
    // Outward: back along the member at its start, on along it at its end.
    let outward = if (along < 0.0) == start { normal } else { normal * -1.0 };
    let along = along.abs().max(1e-12);
    [outward.dot(x) / along, outward.dot(y) / along]
}

/// Which of the eight turns and flips of a square (about the profile's centroid) map the profile
/// onto itself, and whether it is round (onto itself under every turn and flip).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSymmetry {
    /// Bit `i` set: [`SQUARE_MAPS`]`[i]` maps the profile onto itself.
    pub square: u8,
    pub round: bool,
}

/// The turns and flips of a square, as `[xx, xy, yx, yy]`: (x, y) goes to
/// (xx·x + xy·y, yx·x + yy·y). The first four turn; the last four flip.
pub const SQUARE_MAPS: [[f64; 4]; 8] = [
    [1.0, 0.0, 0.0, 1.0],
    [0.0, -1.0, 1.0, 0.0],
    [-1.0, 0.0, 0.0, -1.0],
    [0.0, 1.0, -1.0, 0.0],
    [-1.0, 0.0, 0.0, 1.0],
    [1.0, 0.0, 0.0, -1.0],
    [0.0, 1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0, 0.0],
];

fn apply(m: &[f64; 4], v: [f64; 2]) -> [f64; 2] {
    [m[0] * v[0] + m[1] * v[1], m[2] * v[0] + m[3] * v[1]]
}

/// The symmetries of a profile given as closed loops of points around its centroid (outer
/// boundary and holes), each point within `tol` of where it should be. A profile with more than
/// [`MAX_SYMMETRY_POINTS`] points is taken to have none.
pub fn symmetry_of(loops: &[Vec<solidcraft_geom::Vec2>], tol: f64) -> ProfileSymmetry {
    use solidcraft_geom::Vec2;
    let total: usize = loops.iter().map(Vec::len).sum();
    if total == 0 || total > MAX_SYMMETRY_POINTS {
        return ProfileSymmetry { square: 1, round: false };
    }
    // Round: every loop a circle about the centroid, drawn with enough points to be one.
    let round = loops.iter().all(|l| {
        let r = l.iter().map(|p| p.len()).sum::<f64>() / l.len().max(1) as f64;
        l.len() >= 8 && r > tol && l.iter().all(|p| (p.len() - r).abs() <= tol)
    });
    if round {
        return ProfileSymmetry { square: 0xff, round: true };
    }
    let near_outline = |p: Vec2| {
        loops.iter().any(|l| {
            l.iter().zip(l.iter().cycle().skip(1)).any(|(a, b)| {
                let ab = *b - *a;
                let t = if ab.dot(ab) > 0.0 { ((p - *a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0) } else { 0.0 };
                p.dist(*a + ab * t) <= tol
            })
        })
    };
    let mut square = 0u8;
    for (i, m) in SQUARE_MAPS.iter().enumerate() {
        if loops.iter().flatten().all(|p| near_outline(Vec2::from(apply(m, [p.x, p.y])))) {
            square |= 1 << i;
        }
    }
    ProfileSymmetry { square: square | 1, round: false }
}

/// Above this many outline points a profile's symmetry isn't worked out.
pub const MAX_SYMMETRY_POINTS: usize = 4000;

/// How two members of the same stock compare as parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Likeness {
    /// One can be turned to fit the other.
    Same,
    /// One fits the other's mirror image, and not the other itself.
    Mirror,
    Different,
}

/// Whether members `a` and `b`, cut from the same stock to the same length, have the same end
/// cuts (turned or end for end) or mirror-image ones, with each end's cut within `angle_tol`
/// degrees.
pub fn likeness(a: &MemberCut, b: &MemberCut, angle_tol: f64) -> Likeness {
    let tilt = |l: Lean| l[0].hypot(l[1]).atan().to_degrees();
    let close = |s: Lean, t: Lean| {
        let (n, m) = (solidcraft_geom::Vec3::new(s[0], s[1], 1.0), solidcraft_geom::Vec3::new(t[0], t[1], 1.0));
        n.cross(m).len().atan2(n.dot(m)).to_degrees() <= angle_tol
    };
    let (([a0, a1], a_copes), ([b0, b1], b_copes)) = (as_stock(a), as_stock(b));
    // Coped ends only match ends coped to the same outline (and plain ends plain ones).
    let copes = |end_for_end: bool| {
        let [c0, c1] = if end_for_end { [b_copes[1], b_copes[0]] } else { b_copes };
        same_cope(a_copes[0], c0) && same_cope(a_copes[1], c1)
    };
    if a.symmetry.round || b.symmetry.round {
        // Any turn and flip about the axis: what's left is how far each end leans and which way
        // one end's lean turns from the other's (only where both lean).
        let near = |x: f64, y: f64| (x - y).abs() <= angle_tol;
        let turn = |s: Lean, t: Lean| s[0].mul_add(t[1], -s[1] * t[0]).atan2(s[0].mul_add(t[0], s[1] * t[1])).to_degrees();
        let leans = |s: Lean| tilt(s) > angle_tol;
        let mut out = Likeness::Different;
        for (end_for_end, (c0, c1)) in [(false, (b0, b1)), (true, (b1, b0))] {
            if !(near(tilt(a0), tilt(c0)) && near(tilt(a1), tilt(c1)) && copes(end_for_end)) {
                continue;
            }
            if !(leans(a0) && leans(a1)) {
                return Likeness::Same;
            }
            // End for end reverses the turn and the flip that makes it fit reverses it back.
            let (ta, tb) = (turn(a0, a1), turn(b0, b1));
            let diff = |x: f64| ((x + 540.0).rem_euclid(360.0) - 180.0).abs();
            if diff(ta - tb) <= angle_tol {
                return Likeness::Same;
            }
            if diff(ta + tb) <= angle_tol {
                out = Likeness::Mirror;
            }
        }
        return out;
    }
    let symmetry = a.symmetry.square & b.symmetry.square;
    let mut out = Likeness::Different;
    for (i, m) in SQUARE_MAPS.iter().enumerate().filter(|(i, _)| symmetry & (1 << i) != 0) {
        let flips = i >= 4;
        for end_for_end in [false, true] {
            let (c0, c1) = if end_for_end { (b1, b0) } else { (b0, b1) };
            if close(apply(m, a0), c0) && close(apply(m, a1), c1) && copes(end_for_end) {
                // Turning a member end for end reverses its axis, which only a flip of the
                // profile undoes: a fit by both or by neither is the member itself, by one of
                // them its mirror image.
                if flips == end_for_end {
                    return Likeness::Same;
                }
                out = Likeness::Mirror;
            }
        }
    }
    out
}

/// A member's leans and copes as if cut from its stock unmirrored: a member whose profile is
/// mirrored is that stock's member turned end for end (half a turn about the mirror's axis),
/// which turns each lean's part across that axis the other way.
fn as_stock(m: &MemberCut) -> ([Lean; 2], [&Option<Cope>; 2]) {
    let [l0, l1] = m.leans;
    let [c0, c1] = &m.copes;
    match m.mirror {
        crate::WeldMirror::None => ([l0, l1], [c0, c1]),
        crate::WeldMirror::LeftRight => ([[-l1[0], l1[1]], [-l0[0], l0[1]]], [c1, c0]),
        crate::WeldMirror::UpDown => ([[l1[0], -l1[1]], [l0[0], -l0[1]]], [c1, c0]),
    }
}

/// Whether two ends are both plain, or both coped to outlines of the same size.
fn same_cope(a: &Option<Cope>, b: &Option<Cope>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a.region.area() - b.region.area()).abs() <= 1e-6 * (1.0 + a.region.area().abs()),
        _ => false,
    }
}

/// The cut a plane with unit `normal` makes across a member running along `dir`, whose profile's
/// axes are `x` and `y`: square, a single angle, or a compound of two.
pub fn end_of(normal: solidcraft_geom::Vec3, dir: solidcraft_geom::Vec3, x: solidcraft_geom::Vec3, y: solidcraft_geom::Vec3) -> MemberEnd {
    // Below this an angle reads as none (degrees).
    const FLAT: f64 = 1e-6;
    let along = normal.dot(dir).abs().max(1e-12);
    let mitre = normal.dot(x).abs().atan2(along).to_degrees();
    let bevel = normal.dot(y).abs().atan2(along).to_degrees();
    match (mitre > FLAT, bevel > FLAT) {
        (false, false) => MemberEnd::Square,
        (true, false) => MemberEnd::Mitre { angle: mitre },
        (false, true) => MemberEnd::Mitre { angle: bevel },
        (true, true) => MemberEnd::Compound { mitre, bevel },
    }
}

/// Longest name a library profile takes.
pub const MAX_STOCK_NAME: usize = 128;
/// Most loops (outline and holes) a library profile has.
pub const MAX_STOCK_LOOPS: usize = 64;
/// Most segments in all of a library profile's loops.
pub const MAX_STOCK_SEGMENTS: usize = 4000;
/// Farthest (mm) any point of a library profile lies from its origin.
pub const MAX_STOCK_SIZE: f64 = 1e5;
/// Most stock lengths one profile is bought in.
pub const MAX_STOCK_LENGTHS: usize = 16;
/// Longest stock length (mm) taken.
pub const MAX_STOCK_LENGTH: f64 = 1e6;

/// Why lengths (mm) a stock is bought in can't be used, if they can't: at most
/// [`MAX_STOCK_LENGTHS`] of them, each above zero and at most [`MAX_STOCK_LENGTH`].
pub fn check_lengths(lengths: &[f64]) -> Result<(), String> {
    if lengths.len() > MAX_STOCK_LENGTHS {
        return Err(format!("a stock is bought in at most {MAX_STOCK_LENGTHS} lengths"));
    }
    if !lengths.iter().all(|l| l.is_finite() && *l > 0.0 && *l <= MAX_STOCK_LENGTH) {
        return Err(format!("stock lengths must be above zero and at most {} m", MAX_STOCK_LENGTH / 1000.0));
    }
    Ok(())
}

/// Why a library profile can't be used, if it can't: a name, and an outline of closed loops of
/// finite segments within [`MAX_STOCK_SIZE`], the holes inside the outline, with some area.
pub fn check_stock(name: &str, region: &solidcraft_geom::Region2) -> Result<(), String> {
    use solidcraft_geom::{Seg2, Vec2};
    if name.trim().is_empty() || name.chars().count() > MAX_STOCK_NAME || name.chars().any(char::is_control) {
        return Err(format!("a library profile needs a name of 1…{MAX_STOCK_NAME} characters"));
    }
    let loops = std::iter::once(&region.outer).chain(&region.holes);
    if region.holes.len() >= MAX_STOCK_LOOPS {
        return Err(format!("a library profile has at most {MAX_STOCK_LOOPS} loops"));
    }
    if loops.clone().map(|l| l.segs.len()).sum::<usize>() > MAX_STOCK_SEGMENTS {
        return Err(format!("a library profile has at most {MAX_STOCK_SEGMENTS} segments"));
    }
    let ok = |p: &Vec2| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= MAX_STOCK_SIZE && p.y.abs() <= MAX_STOCK_SIZE;
    let sound = |s: &Seg2| match s {
        Seg2::Line { a, b } => ok(a) && ok(b),
        Seg2::Arc { center, radius, start, sweep } => {
            ok(center)
                && radius.is_finite()
                && *radius > 0.0
                && *radius <= MAX_STOCK_SIZE
                && start.is_finite()
                && sweep.is_finite()
                && sweep.abs() <= 7.0
        }
        Seg2::Cubic { p0, p1, p2, p3 } => [p0, p1, p2, p3].into_iter().all(ok),
        Seg2::Conic { a, apex, b, w } => ok(a) && ok(apex) && ok(b) && w.is_finite() && *w > 0.0 && *w < 1e6,
        Seg2::Bezier { n, p } => (4..=solidcraft_geom::MAX_BEZIER_DEGREE).contains(&usize::from(*n)) && p.iter().all(ok),
    };
    for l in loops.clone() {
        if !l.segs.iter().all(|s| sound(s) && ok(&s.start()) && ok(&s.end())) {
            return Err("a library profile's outline has a segment out of bounds".into());
        }
        if !l.is_closed(1e-4) {
            return Err("a library profile's loops must be closed".into());
        }
    }
    let inside = |h: &solidcraft_geom::Loop2| h.segs.first().is_some_and(|s| region.outer.contains(s.mid()));
    if !region.holes.iter().all(inside) {
        return Err("a library profile's holes must lie inside its outline".into());
    }
    if !(region.area() > 1e-6) {
        return Err("a library profile has no area".into());
    }
    Ok(())
}

/// `region` moved so its centroid is at the origin (as a library keeps its profiles).
pub fn centred(region: &solidcraft_geom::Region2) -> solidcraft_geom::Region2 {
    use solidcraft_geom::Seg2;
    let c = region.centroid();
    let seg = |s: &Seg2| match *s {
        Seg2::Line { a, b } => Seg2::Line { a: a - c, b: b - c },
        Seg2::Arc { center, radius, start, sweep } => Seg2::Arc { center: center - c, radius, start, sweep },
        Seg2::Cubic { p0, p1, p2, p3 } => Seg2::Cubic { p0: p0 - c, p1: p1 - c, p2: p2 - c, p3: p3 - c },
        Seg2::Conic { a, apex, b, w } => Seg2::Conic { a: a - c, apex: apex - c, b: b - c, w },
        Seg2::Bezier { n, p } => Seg2::Bezier { n, p: p.map(|q| q - c) },
    };
    let lp = |l: &solidcraft_geom::Loop2| solidcraft_geom::Loop2 { segs: l.segs.iter().map(seg).collect() };
    solidcraft_geom::Region2 { outer: lp(&region.outer), holes: region.holes.iter().map(lp).collect() }
}

/// Most face directions [`body_box`] tries besides the world axes.
const BOX_DIRECTIONS: usize = 6;

/// The extent of a body's smallest bounding box (mm, longest side first), for the cut list's
/// other bodies: over the mesh `points`, the box square to the world axes or standing on one
/// of the directions its triangles face most (by area), turned about it to the least area.
/// A box over a tessellation sits on the facets, so round sides read a little short.
pub fn body_box(points: &[solidcraft_geom::Vec3], triangles: &[[u32; 3]]) -> [f64; 3] {
    use solidcraft_geom::{Vec2, Vec3};
    let mut faces: Vec<(Vec3, f64)> = Vec::new();
    for t in triangles {
        let (Some(a), Some(b), Some(c)) = (points.get(t[0] as usize), points.get(t[1] as usize), points.get(t[2] as usize)) else { continue };
        let n = (*b - *a).cross(*c - *a);
        let Some(dir) = n.normalized() else { continue };
        // A direction and its opposite stand the box the same way.
        match faces.iter().position(|(d, _)| d.dot(dir).abs() > 1.0 - 1e-9) {
            Some(i) => faces.get_mut(i).into_iter().for_each(|f| f.1 += n.len()),
            None if faces.len() < 4096 => faces.push((dir, n.len())),
            None => {}
        }
    }
    faces.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut best: Option<[f64; 3]> = None;
    let mut keep = |dims: [f64; 3]| {
        if best.is_none_or(|b| dims[0] * dims[1] * dims[2] < b[0] * b[1] * b[2] - 1e-9) {
            best = Some(dims);
        }
    };
    let span = |dir: Vec3| {
        let (lo, hi) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.dot(dir)), hi.max(p.dot(dir))));
        if lo <= hi { hi - lo } else { 0.0 }
    };
    keep([span(Vec3::X), span(Vec3::Y), span(Vec3::Z)]);
    for (n, _) in faces.iter().take(BOX_DIRECTIONS) {
        let u = n.any_perp();
        let v = n.cross(u);
        let flat: Vec<Vec2> = points.iter().map(|p| Vec2::new(p.dot(u), p.dot(v))).collect();
        let [w, h] = least_rectangle(&convex_hull(flat));
        keep([w, h, span(*n)]);
    }
    let mut dims = best.unwrap_or([0.0; 3]);
    dims.sort_by(|a, b| b.total_cmp(a));
    dims
}

/// The convex hull of `points`, counter-clockwise (Andrew's monotone chain).
fn convex_hull(mut points: Vec<solidcraft_geom::Vec2>) -> Vec<solidcraft_geom::Vec2> {
    points.retain(|p| p.is_finite());
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup_by(|a, b| (*a - *b).len2() < 1e-18);
    if points.len() < 3 {
        return points;
    }
    // The lower chain left to right, then the upper one back, each without its last point.
    let chain = |pts: &mut dyn Iterator<Item = solidcraft_geom::Vec2>| {
        let mut out: Vec<solidcraft_geom::Vec2> = Vec::new();
        for p in pts {
            while let (Some(&a), Some(&b)) = (out.len().checked_sub(2).and_then(|i| out.get(i)), out.last()) {
                if (b - a).cross(p - a) > 0.0 {
                    break;
                }
                out.pop();
            }
            out.push(p);
        }
        out.pop();
        out
    };
    let mut hull = chain(&mut points.iter().copied());
    hull.extend(chain(&mut points.iter().rev().copied()));
    hull
}

/// The sides of the least-area rectangle round a convex `hull` (one of its sides lies on a
/// hull edge), longer first.
fn least_rectangle(hull: &[solidcraft_geom::Vec2]) -> [f64; 2] {
    use solidcraft_geom::Vec2;
    let extent = |dir: Vec2| {
        let side = dir.perp();
        let (mut a, mut b) = ((f64::INFINITY, f64::NEG_INFINITY), (f64::INFINITY, f64::NEG_INFINITY));
        for p in hull {
            a = (a.0.min(p.dot(dir)), a.1.max(p.dot(dir)));
            b = (b.0.min(p.dot(side)), b.1.max(p.dot(side)));
        }
        if hull.is_empty() { [0.0, 0.0] } else { [a.1 - a.0, b.1 - b.0] }
    };
    let mut best = extent(Vec2::X);
    for (i, p) in hull.iter().enumerate() {
        let Some(dir) = hull.get((i + 1) % hull.len()).and_then(|q| (*q - *p).normalized()) else { continue };
        let e = extent(dir);
        if e[0] * e[1] < best[0] * best[1] - 1e-9 {
            best = e;
        }
    }
    if best[0] < best[1] { [best[1], best[0]] } else { best }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::{Vec2, Vec3};

    #[test]
    fn end_angles_are_measured_from_square() {
        let (d, x, y) = (Vec3::X, Vec3::Y, Vec3::Z);
        assert_eq!(end_of(Vec3::X, d, x, y), MemberEnd::Square);
        let MemberEnd::Mitre { angle } = end_of(Vec3::new(1.0, 1.0, 0.0) * 0.5f64.sqrt(), d, x, y) else { panic!("a mitre") };
        assert!((angle - 45.0).abs() < 1e-9);
        let n = Vec3::new(1.0, 1.0, 1.0) * (1.0 / 3f64.sqrt());
        let MemberEnd::Compound { mitre, bevel } = end_of(n, d, x, y) else { panic!("a compound cut") };
        assert!((mitre - 45.0).abs() < 1e-9 && (bevel - 45.0).abs() < 1e-9);
    }

    fn outline(points: &[[f64; 2]]) -> Vec<Vec2> {
        points.iter().map(|p| Vec2::from(*p)).collect()
    }

    /// A square tube maps onto itself every way, a rectangle only by half turns and flips across
    /// its axes, an equal angle only across its diagonal, a ring by anything.
    #[test]
    fn profile_symmetries() {
        let square = |h: f64| outline(&[[-h, -h], [h, -h], [h, h], [-h, h]]);
        assert_eq!(symmetry_of(&[square(25.0), square(22.0)], 0.05), ProfileSymmetry { square: 0xff, round: false });
        let rect = outline(&[[-30.0, -15.0], [30.0, -15.0], [30.0, 15.0], [-30.0, 15.0]]);
        assert_eq!(symmetry_of(&[rect], 0.05).square, 0b0011_0101);
        // An equal angle 40 × 40 × 5 about its centroid (11.875 from each outside face).
        let c = 11.875;
        let angle = outline(&[[0.0, 0.0], [40.0, 0.0], [40.0, 5.0], [5.0, 5.0], [5.0, 40.0], [0.0, 40.0]]);
        let angle: Vec<Vec2> = angle.into_iter().map(|p| p - Vec2::new(c, c)).collect();
        assert_eq!(symmetry_of(&[angle], 0.05).square, 0b0100_0001);
        let ring = |r: f64| (0..64).map(|i| Vec2::from_angle(i as f64 * std::f64::consts::TAU / 64.0) * r).collect::<Vec<_>>();
        assert!(symmetry_of(&[ring(25.0), ring(22.0)], 0.05).round);
        assert!(!symmetry_of(&[ring(25.0), square(10.0)], 0.05).round);
    }

    fn cut(leans: [Lean; 2], symmetry: ProfileSymmetry) -> MemberCut {
        MemberCut {
            body: String::new(),
            feature: 0,
            line: String::new(),
            profile: String::new(),
            length: 1.0,
            ends: [MemberEnd::Square; 2],
            axis: [Vec3::ZERO; 2],
            outline: Vec::new(),
            leans,
            symmetry,
            mirror: Default::default(),
            cuts: Default::default(),
            stock: None,
            copes: [None, None],
            bent: None,
            made: MadeBody::default(),
        }
    }

    /// A round member bent at each of `bends` (where it starts along the centreline, its angle and
    /// the way it bends in the profile's frame), `developed` long, with no lead past its ends.
    fn bent(bends: &[(f64, f64, [f64; 2])], developed: f64) -> MemberCut {
        let bends = bends.iter().map(|&(at, angle, toward)| Bend { at, angle, radius: 50.0, toward }).collect();
        let frame = solidcraft_geom::Plane::new(Vec3::ZERO, Vec3::X, Vec3::Y).unwrap();
        MemberCut {
            length: developed,
            bent: Some(Bent { path: Vec::new(), bends, developed, lead: [0.0, 0.0], end_frame: frame, allowance: 0.0, formed: false }),
            ..cut([[0.0; 2]; 2], ProfileSymmetry { square: 0xff, round: true })
        }
    }

    /// A bender's steps: the straight fed before each bend, the stock's turn since the last bend
    /// and the bend; the tail after. Two bends in one plane, the second the other way, turn the
    /// stock half round between them.
    #[test]
    fn bend_steps_feed_turn_and_bend() {
        let quarter = 50.0 * std::f64::consts::FRAC_PI_2;
        let s_bend = bent(&[(100.0, 90.0, [1.0, 0.0]), (100.0 + quarter + 200.0, 90.0, [-1.0, 0.0])], 100.0 + 2.0 * quarter + 200.0 + 150.0);
        let (steps, tail) = bend_steps(s_bend.bent.as_ref().unwrap());
        assert_eq!(steps.len(), 2);
        assert!((steps[0].feed - 100.0).abs() < 1e-9 && steps[0].rotation.abs() < 1e-9);
        assert!((steps[1].feed - 200.0).abs() < 1e-9 && (steps[1].rotation.abs() - 180.0).abs() < 1e-9);
        assert!((tail - 150.0).abs() < 1e-9, "{tail}");
        // The same member bent the same way at both bends (a U) is another part.
        let u_bend = bent(&[(100.0, 90.0, [1.0, 0.0]), (100.0 + quarter + 200.0, 90.0, [1.0, 0.0])], s_bend.length);
        assert!(!same_bends(&s_bend, &u_bend, 0.1, 0.05));
        assert!(same_bends(&s_bend, &s_bend.clone(), 0.1, 0.05));
        // End for end it is the same part: fed 150 first, 200 between.
        let turned = bent(&[(150.0, 90.0, [1.0, 0.0]), (150.0 + quarter + 200.0, 90.0, [-1.0, 0.0])], s_bend.length);
        assert!(same_bends(&s_bend, &turned, 0.1, 0.05));
        // A straight member is never bent alike.
        assert!(!same_bends(&s_bend, &cut([[0.0; 2]; 2], ProfileSymmetry::default()), 0.1, 0.05));
    }

    /// A member of mirrored stock is the stock's member turned end for end about the mirror's
    /// axis: so turned it is the same part, mirrored once more its mirror image.
    #[test]
    fn mirrored_stock_is_turned_stock() {
        let none = ProfileSymmetry { square: 1, round: false };
        let a = cut([[1.0, 0.3], [0.0, 0.0]], none);
        let with = |mirror, leans| MemberCut { mirror, ..cut(leans, none) };
        // Half a turn about the profile's y axis: ends swap, x parts turn round.
        assert_eq!(likeness(&a, &with(crate::WeldMirror::LeftRight, [[0.0, 0.0], [-1.0, 0.3]]), 0.05), Likeness::Same);
        // A reflected in its x = 0 plane: the profile and the x parts mirror.
        assert_eq!(likeness(&a, &with(crate::WeldMirror::LeftRight, [[-1.0, 0.3], [0.0, 0.0]]), 0.05), Likeness::Mirror);
        assert_eq!(likeness(&a, &with(crate::WeldMirror::UpDown, [[0.0, 0.0], [1.0, -0.3]]), 0.05), Likeness::Same);
        assert_eq!(likeness(&a, &with(crate::WeldMirror::UpDown, [[1.0, -0.3], [0.0, 0.0]]), 0.05), Likeness::Mirror);
        assert_eq!(likeness(&a, &with(crate::WeldMirror::LeftRight, [[1.0, 0.3], [0.0, 0.0]]), 0.05), Likeness::Different);
    }

    /// Mitres into two different planes make a handed member; turned end for end it is still
    /// itself, and its mirror image is not.
    #[test]
    fn handed_members() {
        let sq = ProfileSymmetry { square: 0xff, round: false };
        let a = cut([[1.0, 0.0], [0.0, 1.0]], sq);
        assert_eq!(likeness(&a, &cut([[0.0, 1.0], [1.0, 0.0]], sq), 0.05), Likeness::Mirror, "same leans, other ends");
        assert_eq!(likeness(&a, &cut([[1.0, 0.0], [0.0, -1.0]], sq), 0.05), Likeness::Mirror);
        assert_eq!(likeness(&a, &cut([[0.0, -1.0], [1.0, 0.0]], sq), 0.05), Likeness::Same, "a quarter turn");
        assert_eq!(likeness(&a, &cut([[1.0, 0.0], [1.0, 0.0]], sq), 0.05), Likeness::Different);
        // With no symmetry, a member only fits itself the way round it was made.
        let none = ProfileSymmetry { square: 1, round: false };
        let b = cut([[1.0, 0.0], [0.5, 0.0]], none);
        assert_eq!(likeness(&b, &b, 0.05), Likeness::Same);
        assert_eq!(likeness(&b, &cut([[0.5, 0.0], [1.0, 0.0]], none), 0.05), Likeness::Mirror, "end for end is its mirror");
        let round = ProfileSymmetry { square: 0xff, round: true };
        let r = cut([[1.0, 0.0], [0.0, 1.0]], round);
        let turned = |t: f64| {
            let (s, c) = t.sin_cos();
            cut([[c, s], [-s, c]], round)
        };
        assert_eq!(likeness(&r, &turned(0.3), 0.05), Likeness::Same);
        assert_eq!(likeness(&r, &cut([[1.0, 0.0], [0.0, -1.0]], round), 0.05), Likeness::Mirror);
    }

    /// A block's box is its own size however it is turned; a pipe's is its length and diameter
    /// (a little under, over the facets).
    #[test]
    fn body_boxes_fit_turned_bodies() {
        let block = |turn: f64, tilt: f64| {
            let (c, s, ct, st) = (turn.cos(), turn.sin(), tilt.cos(), tilt.sin());
            let place = |x: f64, y: f64, z: f64| {
                let (x, y) = (x * c - y * s, x * s + y * c);
                Vec3::new(x, y * ct - z * st, y * st + z * ct) + Vec3::new(40.0, -7.0, 3.0)
            };
            let mut pts = Vec::new();
            for i in 0..8 {
                pts.push(place(if i & 1 == 0 { 0.0 } else { 100.0 }, if i & 2 == 0 { 0.0 } else { 40.0 }, if i & 4 == 0 { 0.0 } else { 10.0 }));
            }
            let tris = vec![
                [0, 2, 1],
                [1, 2, 3],
                [4, 5, 6],
                [5, 7, 6],
                [0, 1, 4],
                [1, 5, 4],
                [2, 6, 3],
                [3, 6, 7],
                [0, 4, 2],
                [2, 4, 6],
                [1, 3, 5],
                [3, 7, 5],
            ];
            body_box(&pts, &tris)
        };
        for (turn, tilt) in [(0.0, 0.0), (0.5, 0.0), (0.3, 0.7), (1.2, -2.0)] {
            let d = block(turn, tilt);
            assert!((d[0] - 100.0).abs() < 1e-6 && (d[1] - 40.0).abs() < 1e-6 && (d[2] - 10.0).abs() < 1e-6, "{turn} {tilt}: {d:?}");
        }
        // A pipe Ø50 × 300 along (1, 1, 0), 64 facets round.
        let (axis, u) = (Vec3::new(1.0, 1.0, 0.0) * 0.5f64.sqrt(), Vec3::Z);
        let v = axis.cross(u);
        let mut pts = Vec::new();
        for i in 0..64 {
            let a = i as f64 / 64.0 * std::f64::consts::TAU;
            let r = u * (25.0 * a.cos()) + v * (25.0 * a.sin());
            pts.push(r);
            pts.push(r + axis * 300.0);
        }
        let mut tris = Vec::new();
        for i in 0..64u32 {
            let (a, b) = (2 * i, 2 * ((i + 1) % 64));
            tris.push([a, b, a + 1]);
            tris.push([b, b + 1, a + 1]);
        }
        for i in 1..63u32 {
            tris.push([0, 2 * (i + 1), 2 * i]);
            tris.push([1, 2 * i + 1, 2 * (i + 1) + 1]);
        }
        let d = body_box(&pts, &tris);
        assert!((d[0] - 300.0).abs() < 1e-6 && d[1] > 49.8 && d[1] <= 50.0 + 1e-9 && d[2] > 49.8 && d[2] <= 50.0 + 1e-9, "{d:?}");
    }
}
