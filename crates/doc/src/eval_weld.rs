//! Evaluation of weldment Structural Members: a closed sketch profile extruded along each
//! straight line of a skeleton sketch, one body per line, with each end cut for its joint (a
//! mitre or a butt where members meet, a butt where it ends on another member, of this feature
//! or an earlier one) and square at free ends.

use super::*;
use crate::{Expr, WeldCorner, WeldJoint, WeldLocate, WeldMirror};

/// Most skeleton lines one Structural Member takes.
pub const MAX_MEMBERS: usize = 1000;

/// Largest end allowance (mm).
const MAX_ALLOWANCE: f64 = 1.0e4;

/// Skeleton ends closer than this are one joint.
const JOINT_TOL: f64 = 1e-3;

/// Members meeting at less than this angle (radians) fold back on each other: no mitre is cut.
const MIN_JOINT_ANGLE: f64 = 0.5 * std::f64::consts::PI / 180.0;

/// One member of the skeleton, from `a` to `b` (world, in the feature's frame): straight, or bent
/// along `path`; a ring where `path` closes on itself (`a` its seam).
#[derive(Clone, Debug)]
struct Line {
    a: Vec3,
    b: Vec3,
    /// The direction the profile's y axis leans toward (the skeleton sketch's normal for a
    /// planar sketch, world Z for a 3D one).
    up: Vec3,
    /// A bent member's centreline from `a` to `b`: lines and arcs, each running on into the next
    /// (empty for a straight member).
    path: Vec<kernel::PathSeg>,
    /// Whether part of `path` follows a free-form curve (as short lines): the member is formed to
    /// that shape rather than bent at bends.
    formed: bool,
}

/// How one end of a member is cut: the plane it is cut on, the member kept on its positive side.
#[derive(Clone, Copy, Debug)]
struct EndCut {
    at: Vec3,
    normal: Vec3,
}

/// A member as its joints see it: its line, its direction, its profile's axes and its profile's
/// outline as offsets from the line (world vectors square to it), at its start and at its end
/// (the same for a straight member), and the profile's outline in its own plane (to carry along a
/// bent member).
#[derive(Clone)]
struct Member<'a> {
    id: &'a str,
    /// The member's group (0: the feature's own path).
    group: usize,
    line: Line,
    dir: Vec3,
    axes: (Vec3, Vec3),
    outline: Vec<Vec3>,
    /// At its end `b`: its direction (on from `a`'s side), the profile's axes and outline.
    end_dir: Vec3,
    end_axes: (Vec3, Vec3),
    end_outline: Vec<Vec3>,
    /// The profile's outline in the profile's plane, the point on the line at the origin.
    profile: Vec<Vec2>,
}

impl<'a> Member<'a> {
    fn straight(id: &'a str, group: usize, line: Line, dir: Vec3, axes: (Vec3, Vec3), outline: Vec<Vec3>) -> Member<'a> {
        let profile = outline.iter().map(|o| Vec2::new(o.dot(axes.0), o.dot(axes.1))).collect();
        Member { id, group, line, dir, axes, end_dir: dir, end_axes: axes, end_outline: outline.clone(), outline, profile }
    }

    /// Member `id` along `line` with its profile's frame `frame` at its start and its profile's
    /// outline `outline` (in that frame, `anchor` on the line).
    fn along(id: &'a str, group: usize, line: &Line, frame: &Plane, anchor: Vec2, outline: &[Vec2]) -> Member<'a> {
        Member::carried(id, group, line.clone(), (frame.x, frame.y), outline.iter().map(|p| *p - anchor).collect())
    }

    /// Member `id` along `line`, its profile's axes `axes` at its start and its outline `profile`
    /// (in the profile's plane, the point on the line at the origin); a bent member's axes are
    /// carried along its centreline (see [`carry`]).
    fn carried(id: &'a str, group: usize, line: Line, axes: (Vec3, Vec3), profile: Vec<Vec2>) -> Member<'a> {
        let offsets = |(x, y): (Vec3, Vec3)| profile.iter().map(|p| x * p.x + y * p.y).collect::<Vec<_>>();
        let dir = line.tangents().map_or(Vec3::Z, |t| t.0);
        let (end_dir, end_axes) = carry(&line, axes, None).unwrap_or((dir, axes));
        Member { id, group, dir, axes, outline: offsets(axes), end_dir, end_axes, end_outline: offsets(end_axes), line, profile }
    }

    /// The profile's outline as offsets from the line, its axes being `axes`.
    fn offsets(&self, (x, y): (Vec3, Vec3)) -> Vec<Vec3> {
        self.profile.iter().map(|p| x * p.x + y * p.y).collect()
    }

    /// Its end at its start `a` or its end `b`, and its direction into the member there.
    fn end(&self, start: bool) -> (Vec3, Vec3) {
        if start { (self.line.a, self.dir) } else { (self.line.b, -self.end_dir) }
    }

    /// The profile's outline at its end nearest `p`.
    fn outline_at(&self, p: Vec3) -> &[Vec3] {
        if self.line.b.dist(p) < self.line.a.dist(p) { &self.end_outline } else { &self.outline }
    }

    /// Its direction and profile outline at `p`, one of its ends or a point of its line (`None`
    /// where a bent member's line doesn't pass `p`).
    fn across(&self, p: Vec3) -> Option<(Vec3, Vec<Vec3>)> {
        if self.line.path.is_empty() || self.line.a.dist(p) < JOINT_TOL {
            return Some((self.dir, self.outline.clone()));
        }
        if self.line.b.dist(p) < JOINT_TOL {
            return Some((self.end_dir, self.end_outline.clone()));
        }
        let (d, axes) = carry(&self.line, self.axes, Some(p))?;
        Some((d, self.offsets(axes)))
    }

    /// How far the member's profile reaches from its line toward `n` (at its start).
    fn extent(&self, n: Vec3) -> f64 {
        extent_of(&self.outline, n)
    }

    /// The member's overall length from `start` to `end`, between the long points: the outline
    /// point each cut leaves furthest out (a bent member's along its centreline).
    fn long_point_length(&self, start: &EndCut, end: &EndCut) -> f64 {
        self.long_point_length_coped(start, end, &[None, None])
    }

    /// How far a bent member's stock reaches past each end of its centreline, cut on `start` and
    /// `end` (and coped there by `copes`): each end as a straight member along its direction
    /// there.
    fn leads(&self, start: &EndCut, end: &EndCut, copes: &[Option<crate::weld::Cope>; 2]) -> [f64; 2] {
        let (a, b, up) = (self.line.a, self.line.b, self.line.up);
        let first = Member::straight(self.id, 0, Line::straight(a, a + self.dir, up), self.dir, self.axes, self.outline.clone());
        let last = Member::straight(self.id, 0, Line::straight(b - self.end_dir, b, up), self.end_dir, self.end_axes, self.end_outline.clone());
        let (s, e) = (first.reach(start, copes[0].as_ref(), true), last.reach(end, copes[1].as_ref(), false));
        [if s.is_finite() { -s } else { 0.0 }, if e.is_finite() { e - 1.0 } else { 0.0 }]
    }

    /// [`Self::long_point_length`] with either end coped.
    fn long_point_length_coped(&self, start: &EndCut, end: &EndCut, copes: &[Option<crate::weld::Cope>; 2]) -> f64 {
        if !self.line.path.is_empty() {
            let [l0, l1] = self.leads(start, end, copes);
            return self.line.length() + l0 + l1;
        }
        let first = self.reach(start, copes[0].as_ref(), true);
        let last = self.reach(end, copes[1].as_ref(), false);
        if first.is_finite() && last.is_finite() { last - first } else { self.line.a.dist(self.line.b) }
    }
    /// How far along the line (from its start) the stock reaches at its start (the least) or end
    /// (the most), cut on `c` and, where coped, short of the outline it fits.
    fn reach(&self, c: &EndCut, cope: Option<&crate::weld::Cope>, start: bool) -> f64 {
        let length = self.line.a.dist(self.line.b);
        // Where the cut meets the edge of the stock through outline offset `o`, along the line.
        let along = |o: Vec3| {
            let n = c.normal.dot(self.dir);
            let flat = if n.abs() < 1e-12 { 0.0 } else { (c.normal.dot(c.at - self.line.a) - c.normal.dot(o)) / n };
            // A coped edge stops where it first meets the outline it fits, coming from the
            // member's other end.
            let Some(cope) = cope else { return flat };
            let f = &cope.frame;
            let q0 = Vec2::new(f.x.dot(self.line.a + o - f.origin), f.y.dot(self.line.a + o - f.origin));
            let dq = Vec2::new(f.x.dot(self.dir), f.y.dot(self.dir));
            let poly = cope.region.outer.polyline(1e-2);
            let hits = poly.iter().zip(poly.iter().cycle().skip(1)).filter_map(|(p, q)| {
                let e = *q - *p;
                let den = dq.cross(e);
                if den.abs() < 1e-12 {
                    return None;
                }
                let s = (*p - q0).cross(e) / den;
                let u = (*p - q0).cross(dq) / den;
                (0.0..=1.0).contains(&u).then_some(s)
            });
            if start { hits.filter(|s| *s < length).fold(flat, f64::max) } else { hits.filter(|s| *s > 0.0).fold(flat, f64::min) }
        };
        if start {
            self.outline.iter().map(|o| along(*o)).fold(f64::INFINITY, f64::min)
        } else {
            self.outline.iter().map(|o| along(*o)).fold(f64::NEG_INFINITY, f64::max)
        }
    }

    /// Whether the member's line starts or ends at `p` (a ring has no ends).
    fn ends_at(&self, p: Vec3) -> bool {
        !self.line.closed() && (self.line.a.dist(p) < JOINT_TOL || self.line.b.dist(p) < JOINT_TOL)
    }

    /// Whether `p` lies on the member's line (straight, along its bends or round a ring) strictly
    /// between its ends.
    fn passes_through(&self, p: Vec3) -> bool {
        self.line.passes_through(p)
    }
}

/// How far an outline (offsets from a member's line) reaches toward `n`.
fn extent_of(outline: &[Vec3], n: Vec3) -> f64 {
    outline.iter().map(|o| o.dot(n)).fold(0.0, f64::max)
}

/// `v` turned by `angle` about the unit axis `k` (right hand).
fn turned(v: Vec3, k: Vec3, angle: f64) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c))
}

/// `v` turned the least way that takes unit direction `from` to unit direction `to`.
fn aligned(v: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let c = from.cross(to);
    match c.normalized() {
        Some(k) if c.len() > 1e-12 => turned(v, k, c.len().atan2(from.dot(to))),
        _ => v,
    }
}

/// The profile's axes `axes` at the start of `line` carried along it to `upto`, a point on it, or
/// to its end: turned with each arc, and from one piece's direction to the next's the least way
/// (as the kernel's sweep turns the profile along short lines). Its direction there and the axes;
/// `None` when `upto` is not on the line.
fn carry(line: &Line, axes: (Vec3, Vec3), upto: Option<Vec3>) -> Option<(Vec3, (Vec3, Vec3))> {
    let (t0, _) = line.tangents()?;
    if line.path.is_empty() {
        return match upto {
            Some(p) if p.dist_to_segment(line.a, line.b) >= JOINT_TOL => None,
            _ => Some((t0, axes)),
        };
    }
    let (mut t, (mut x, mut y)) = (t0, axes);
    for seg in &line.path {
        let (_, _, d0, d1) = seg_ends(seg)?;
        (x, y) = (aligned(x, t, d0), aligned(y, t, d0));
        match *seg {
            kernel::PathSeg::Line { a, b } => {
                if upto.is_some_and(|p| p.dist_to_segment(a, b) < JOINT_TOL) {
                    return Some((d0, (x, y)));
                }
            }
            kernel::PathSeg::Arc { axis, angle, .. } => {
                let k = axis.normalized()?;
                if let Some(turn) = upto.and_then(|p| arc_turn(seg, p)) {
                    return Some((turned(d0, k, turn), (turned(x, k, turn), turned(y, k, turn))));
                }
                (x, y) = (turned(x, k, angle), turned(y, k, angle));
            }
        }
        t = d1;
    }
    upto.is_none().then_some((t, (x, y)))
}

/// How far round arc `s` (from its start) `p` lies, where it lies on the arc.
fn arc_turn(s: &kernel::PathSeg, p: Vec3) -> Option<f64> {
    let kernel::PathSeg::Arc { a, center, axis, angle } = *s else { return None };
    let k = axis.normalized()?;
    let (u, v) = (a - center, p - center);
    let r = u.len();
    if v.dot(k).abs() > JOINT_TOL || (v.len() - r).abs() > JOINT_TOL || r < JOINT_TOL {
        return None;
    }
    // How far round from `a` (right hand about `k`).
    let turn = k.dot(u.cross(v)).atan2(u.dot(v)).rem_euclid(std::f64::consts::TAU);
    if turn <= angle + JOINT_TOL / r {
        Some(turn.min(angle))
    } else if std::f64::consts::TAU - turn <= JOINT_TOL / r {
        Some(0.0)
    } else {
        None
    }
}

/// The point of path segment `s` nearest `p`.
fn seg_nearest(s: &kernel::PathSeg, p: Vec3) -> Vec3 {
    match *s {
        kernel::PathSeg::Line { a, b } => {
            let d = b - a;
            let l2 = d.dot(d);
            if l2 < 1e-24 { a } else { a + d * ((p - a).dot(d) / l2).clamp(0.0, 1.0) }
        }
        kernel::PathSeg::Arc { a, center, axis, angle } => {
            let Some(k) = axis.normalized() else { return a };
            let u = a - center;
            let v = p - center;
            let flat = v - k * v.dot(k);
            let turn = k.dot(u.cross(flat)).atan2(u.dot(flat)).rem_euclid(std::f64::consts::TAU);
            // Past its end: whichever end is nearer round the circle.
            let turn = if turn <= angle {
                turn
            } else if turn - angle < std::f64::consts::TAU - turn {
                angle
            } else {
                0.0
            };
            center + turned(u, k, turn)
        }
    }
}

/// A path segment's start and end points, and its unit directions there.
fn seg_ends(s: &kernel::PathSeg) -> Option<(Vec3, Vec3, Vec3, Vec3)> {
    match *s {
        kernel::PathSeg::Line { a, b } => {
            let d = (b - a).normalized()?;
            Some((a, b, d, d))
        }
        kernel::PathSeg::Arc { a, center, axis, angle } => {
            let k = axis.normalized()?;
            let end = center + turned(a - center, k, angle);
            Some((a, end, k.cross(a - center).normalized()?, k.cross(end - center).normalized()?))
        }
    }
}

fn seg_length(s: &kernel::PathSeg) -> f64 {
    match *s {
        kernel::PathSeg::Line { a, b } => a.dist(b),
        kernel::PathSeg::Arc { a, center, angle, .. } => a.dist(center) * angle.abs(),
    }
}

/// The segment run the other way.
fn seg_reversed(s: &kernel::PathSeg) -> kernel::PathSeg {
    match *s {
        kernel::PathSeg::Line { a, b } => kernel::PathSeg::Line { a: b, b: a },
        kernel::PathSeg::Arc { a, center, axis, angle } => {
            let k = axis.normalized().unwrap_or(Vec3::Z);
            kernel::PathSeg::Arc { a: center + turned(a - center, k, angle), center, axis: -k, angle }
        }
    }
}

/// Path segments meeting at less than this angle (radians) run on into each other: the kernel's
/// sweep follows them without a corner.
const TANGENT: f64 = 1e-3;

impl Line {
    fn straight(a: Vec3, b: Vec3, up: Vec3) -> Line {
        Line { a, b, up, path: Vec::new(), formed: false }
    }

    /// Whether it is a ring: a bent member's centreline closing on itself.
    fn closed(&self) -> bool {
        !self.path.is_empty() && self.a.dist(self.b) < JOINT_TOL
    }

    /// Its directions at `a` and at `b`, both running on from `a` toward `b`.
    fn tangents(&self) -> Option<(Vec3, Vec3)> {
        match (self.path.first(), self.path.last()) {
            (Some(f), Some(l)) => Some((seg_ends(f)?.2, seg_ends(l)?.3)),
            _ => {
                let d = (self.b - self.a).normalized()?;
                Some((d, d))
            }
        }
    }

    /// Its length along its centreline.
    fn length(&self) -> f64 {
        if self.path.is_empty() { self.a.dist(self.b) } else { self.path.iter().map(seg_length).sum() }
    }

    /// The point of its centreline nearest `p`.
    fn nearest(&self, p: Vec3) -> Vec3 {
        if self.path.is_empty() {
            return seg_nearest(&kernel::PathSeg::Line { a: self.a, b: self.b }, p);
        }
        self.path.iter().map(|s| seg_nearest(s, p)).min_by(|q, r| q.dist(p).total_cmp(&r.dist(p))).unwrap_or(self.a)
    }

    /// The point of its centreline nearest the line through `p` along unit `d` (where a member
    /// ending at `p`, running on along `d`, meets it).
    fn nearest_to_line(&self, p: Vec3, d: Vec3) -> Vec3 {
        let off = |q: Vec3| {
            let v = q - p;
            (v - d * v.dot(d)).len()
        };
        let line = [kernel::PathSeg::Line { a: self.a, b: self.b }];
        let segs = if self.path.is_empty() { &line[..] } else { &self.path[..] };
        let mut best: Option<Vec3> = None;
        for s in segs {
            let q = match *s {
                kernel::PathSeg::Line { a, b } => {
                    // Where the segment comes closest to the line.
                    let e = b - a;
                    let w = a - p;
                    let (ee, ed, ew, dw) = (e.dot(e), e.dot(d), e.dot(w), d.dot(w));
                    let den = ee - ed * ed;
                    let t = if den.abs() < 1e-12 { 0.0 } else { ((ed * dw - ew) / den).clamp(0.0, 1.0) };
                    a + e * t
                }
                kernel::PathSeg::Arc { a, center, axis, angle } => {
                    let Some(k) = axis.normalized() else { continue };
                    let at = |t: f64| center + turned(a - center, k, t);
                    // The nearest of samples round it, then narrowed down between its neighbours.
                    const N: usize = 64;
                    let step = angle / N as f64;
                    let i = (0..=N).min_by(|&i, &j| off(at(step * i as f64)).total_cmp(&off(at(step * j as f64)))).unwrap_or(0);
                    let (mut lo, mut hi) = ((step * i as f64 - step).max(0.0), (step * i as f64 + step).min(angle));
                    for _ in 0..60 {
                        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
                        if off(at(m1)) < off(at(m2)) { hi = m2 } else { lo = m1 }
                    }
                    at((lo + hi) / 2.0)
                }
            };
            if best.is_none_or(|b| off(q) < off(b)) {
                best = Some(q);
            }
        }
        best.unwrap_or(self.a)
    }

    /// Whether `p` lies on the line (along a bent member's centreline) strictly between its ends
    /// (anywhere round a ring).
    fn passes_through(&self, p: Vec3) -> bool {
        if !self.path.is_empty() {
            return (self.closed() || (self.a.dist(p) >= JOINT_TOL && self.b.dist(p) >= JOINT_TOL))
                && self.path.iter().any(|s| match *s {
                    kernel::PathSeg::Line { a, b } => p.dist_to_segment(a, b) < JOINT_TOL,
                    kernel::PathSeg::Arc { .. } => arc_turn(s, p).is_some(),
                });
        }
        let length = self.a.dist(self.b);
        let Some(dir) = (self.b - self.a).normalized() else { return false };
        let s = (p - self.a).dot(dir);
        s > JOINT_TOL && s < length - JOINT_TOL && (self.a + dir * s).dist(p) < JOINT_TOL
    }
}

/// How the feature joins its members: each group's corner, its joints joined otherwise, the gap
/// and the members of earlier features its members butt against.
struct Rules<'a> {
    corners: Vec<WeldCorner>,
    joints: &'a [WeldJoint],
    gap: f64,
    earlier: &'a [Member<'a>],
}

pub(super) fn eval(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState, warning: &mut Option<String>) -> Result<()> {
    let FeatureKind::WeldMember { sketch, profiles, stock, angle, locate, mirror, corner, groups, joints, gap, join_earlier, end_allowance, .. } =
        &f.kind
    else {
        return Err(DocError::Invalid(format!("{} is not a structural member", f.name)));
    };
    let keys = f.kind.weld_keys();
    if keys.is_empty() || keys.len() > MAX_MEMBERS || groups.iter().any(|g| g.path.is_empty()) {
        return Err(DocError::Invalid(format!("a structural member needs 1…{MAX_MEMBERS} skeleton lines, and each group one or more")));
    }
    if let Some(k) = keys.iter().enumerate().find_map(|(i, k)| keys.iter().take(i).any(|o| o == k).then_some(k)) {
        return Err(DocError::Invalid(format!("skeleton line `{k}` is listed twice")));
    }
    check_joints(&keys, joints)?;
    // The stock is named after the library profile, or the profile's sketch.
    let (region, profile) = match (stock, sketch) {
        (Some(stock), _) => {
            crate::weld::check_stock(&stock.name, &stock.region).map_err(DocError::Invalid)?;
            (stock.region.clone(), stock.name.clone())
        }
        (None, Some(sketch)) => {
            let ss = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch} (it must come earlier in the timeline)")))?;
            let region = profile_region(&select_profiles(ss, profiles.as_ref().unwrap_or(&ProfileSel::All))?)?;
            (region, doc.feature(sketch.0).map(|s| s.name.clone()).unwrap_or_default())
        }
        (None, None) => return Err(DocError::Invalid("a structural member needs a profile: a sketch or a library profile".into())),
    };
    let turn_of = |angle: &Option<Expr>| -> Result<f64> {
        let turn = match angle {
            Some(a) => val(vals, a, Kind::Angle)?,
            None => 0.0,
        };
        if !turn.is_finite() {
            return Err(DocError::Invalid("the profile angle must be finite".into()));
        }
        Ok(turn)
    };
    let turn = turn_of(angle)?;
    // Each group's turn, corner and placed profile; the feature's own path first.
    let mut all: Vec<(f64, WeldCorner, (WeldLocate, WeldMirror))> = vec![(turn, *corner, (*locate, *mirror))];
    for g in groups {
        let turn = if g.angle.is_some() { turn_of(&g.angle)? } else { turn };
        all.push((turn, g.corner.unwrap_or(*corner), (g.locate.unwrap_or(*locate), g.mirror.unwrap_or(*mirror))));
    }
    let (lines, key_of, groups_of) = skeleton(doc, st, &f.kind, f.component)?;
    // Groups placed alike share one placed profile.
    let mut placings: Vec<((WeldLocate, WeldMirror), Placed)> = Vec::new();
    let mut group_placing = Vec::with_capacity(all.len());
    for (_, _, how) in &all {
        let p = match placings.iter().position(|(h, _)| h == how) {
            Some(p) => p,
            None => {
                placings.push((*how, placed(&region, how.0, how.1)?));
                placings.len() - 1
            }
        };
        group_placing.push(p);
    }
    let turns: Vec<f64> = groups_of.iter().map(|&g| all.get(g).map_or(turn, |a| a.0)).collect();
    let gap = match gap {
        Some(g) => val(vals, g, Kind::Length)?,
        None => 0.0,
    };
    if !(gap.is_finite() && gap >= 0.0) {
        return Err(DocError::Invalid("the weld gap must be zero or more".into()));
    }
    let allowance = match end_allowance {
        Some(e) => val(vals, e, Kind::Length)?,
        None => 0.0,
    };
    if !(allowance.is_finite() && (0.0..=MAX_ALLOWANCE).contains(&allowance)) {
        return Err(DocError::Invalid(format!("the end allowance must be from 0 to {MAX_ALLOWANCE} mm")));
    }
    // The profile's own symmetries (about its centroid, before any mirror): the cut list compares
    // a mirrored member as the unmirrored stock turned end for end.
    let centre = region.centroid();
    let symmetry = crate::weld::symmetry_of(
        &std::iter::once(&region.outer).chain(&region.holes).map(|l| l.polyline(1e-2).into_iter().map(|p| p - centre).collect()).collect::<Vec<_>>(),
        0.05,
    );
    let place = |k: usize| {
        groups_of
            .get(k)
            .and_then(|&g| group_placing.get(g))
            .and_then(|&p| placings.get(p))
            .map(|(_, p)| p)
            .ok_or_else(|| DocError::Invalid("member profile".into()))
    };
    let mut members = Vec::with_capacity(lines.len());
    for (k, ((line, &group), &turn)) in lines.iter().zip(&groups_of).zip(&turns).enumerate() {
        let id = key_of.get(k).and_then(|&i| keys.get(i)).ok_or_else(|| DocError::Invalid("member key".into()))?;
        let pl = place(k)?;
        let frame = member_frame(line, pl.anchor, turn)?;
        members.push(Member::along(id, group, line, &frame, pl.anchor, &pl.outline));
    }
    // The members of earlier features in this component that are still there, as they were made.
    let earlier_cuts: Vec<crate::weld::MemberCut> = if *join_earlier {
        st.welds
            .iter()
            .filter(|w| w.feature != f.id && st.body(&w.body).is_some() && doc.feature(w.feature).is_some_and(|o| o.component == f.component))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let earlier: Vec<Member> = earlier_cuts.iter().filter_map(recorded).collect();
    let rules = Rules { corners: all.iter().map(|g| g.1).collect(), joints, gap, earlier: &earlier };
    let mut notes = Vec::new();
    let mut made = Vec::with_capacity(lines.len());
    for (k, ((line, m), &turn)) in lines.iter().zip(&members).zip(&turns).enumerate() {
        let start = end_cut(&members, &rules, k, true, &mut notes);
        let end = end_cut(&members, &rules, k, false, &mut notes);
        let pl = place(k)?;
        let body = member(line, &pl.region, pl.anchor, turn, pl.reach, start, end)?;
        let ((x, y), (xe, ye)) = (m.axes, m.end_axes);
        let ends = [crate::weld::end_of(start.normal, m.dir, x, y), crate::weld::end_of(end.normal, m.end_dir, xe, ye)];
        let leans = [crate::weld::lean_of(start.normal, m.dir, x, y, true), crate::weld::lean_of(end.normal, m.end_dir, xe, ye, false)];
        let cuts = [start, end].map(|c| crate::weld::CutPlane { at: c.at, normal: c.normal });
        let frame = member_frame(line, pl.anchor, turn)?;
        let bent = bent_record(m, &start, &end, &[None, None], pl.anchor, allowance);
        if bent.is_some()
            && allowance <= 0.0
            && !line.closed()
            && [line.path.first(), line.path.last()].iter().any(|s| matches!(s, Some(kernel::PathSeg::Arc { .. })))
        {
            notes.push(format!(
                "`{}` is bent right to its end: bending needs straight stock to hold there (an end allowance adds it, to trim off after)",
                m.id
            ));
        }
        made.push((Traced::unnamed(body), m.long_point_length(&start, &end), ends, leans, cuts, frame, m, pl, bent));
    }
    st.welds.retain(|w| w.feature != f.id);
    for (k, (t, length, ends, leans, cuts, frame, m, pl, bent)) in made.into_iter().enumerate() {
        let name = new_name(st, f, k);
        st.welds.push(crate::weld::MemberCut {
            body: name.clone(),
            feature: f.id,
            line: m.id.to_string(),
            profile: profile.clone(),
            length,
            ends,
            axis: [m.line.a, m.line.b],
            outline: m.outline.clone(),
            leans,
            symmetry,
            mirror: pl.mirror,
            cuts,
            stock: Some(crate::weld::Stock { region: pl.region.clone(), frame }),
            copes: [None, None],
            bent,
            made: crate::weld::MadeBody(Some(t.body.clone())),
        });
        st.bodies.push(ModelBody::traced_new(name, t, f.id));
    }
    for j in joints.iter() {
        let [a, b] = &j.members;
        let meet = |x: &str, y: &str| {
            let (Some(x), Some(y)) = (members.iter().find(|m| m.id == x), members.iter().find(|m| m.id == y)) else { return false };
            [x.line.a, x.line.b].iter().any(|p| y.line.a.dist(*p) < JOINT_TOL || y.line.b.dist(*p) < JOINT_TOL)
        };
        if !meet(a, b) {
            notes.push(format!("the joint of `{a}` and `{b}` changes nothing: they don't meet at their ends"));
        }
    }
    notes.sort();
    notes.dedup();
    if !notes.is_empty() {
        append_warning(warning, notes.join("; "));
    }
    Ok(())
}

/// Joints name two different members (by key), and a butt's through member is one of them.
fn check_joints(path: &[String], joints: &[WeldJoint]) -> Result<()> {
    if joints.len() > MAX_MEMBERS * 2 {
        return Err(DocError::Invalid(format!("a structural member takes at most {} joints", MAX_MEMBERS * 2)));
    }
    for (i, j) in joints.iter().enumerate() {
        let [a, b] = &j.members;
        for m in [a, b] {
            if !path.contains(m) {
                return Err(DocError::Invalid(format!("joint {}: `{m}` is not a member of this feature", i + 1)));
            }
        }
        if a == b {
            return Err(DocError::Invalid(format!("joint {}: a member can't be joined to itself", i + 1)));
        }
        if let Some(t) = &j.through
            && t != a
            && t != b
        {
            return Err(DocError::Invalid(format!("joint {}: the member running through, `{t}`, must be one of its two members", i + 1)));
        }
        if joints.iter().take(i).any(|o| o.members.contains(a) && o.members.contains(b)) {
            return Err(DocError::Invalid(format!("joint {}: `{a}` and `{b}` are joined twice", i + 1)));
        }
    }
    Ok(())
}

/// A corner of a Structural Member: two of its members ending at one point (the first two in
/// key order where more end there), with no member of the feature running through it.
#[derive(Clone, Debug, PartialEq)]
pub struct WeldCornerAt {
    pub members: [String; 2],
    pub at: Vec3,
    /// How it is joined: its joint's corner, else the later group's.
    pub corner: WeldCorner,
    /// For a butt, the member that runs through.
    pub through: Option<String>,
    /// Whether one of the feature's `joints` sets it.
    pub own: bool,
}

/// The corners of Structural Member `kind`, authored in component `component`, its skeleton read
/// from `st`. Members of earlier features are not looked at: an end through one of them is
/// butted against it whatever the corner says.
pub fn corners(doc: &Document, st: &ModelState, kind: &FeatureKind, component: u64) -> Result<Vec<WeldCornerAt>> {
    let FeatureKind::WeldMember { corner, groups, joints, .. } = kind else {
        return Err(DocError::Invalid("not a structural member".into()));
    };
    let keys = kind.weld_keys();
    let (lines, key_of, groups_of) = skeleton(doc, st, kind, component)?;
    let group_corner = |g: usize| if g == 0 { *corner } else { groups.get(g - 1).and_then(|g| g.corner).unwrap_or(*corner) };
    // Each member's directions into it from its start and from its end.
    let dirs: Vec<(Vec3, Vec3)> = lines.iter().map(|l| l.tangents().map_or((Vec3::ZERO, Vec3::ZERO), |(t0, t1)| (t0, -t1))).collect();
    let mut out = Vec::new();
    for (k, (line, &(in_a, in_b))) in lines.iter().zip(&dirs).enumerate() {
        if line.closed() {
            continue;
        }
        for (at, into) in [(line.a, in_a), (line.b, in_b)] {
            if lines.iter().any(|l| l.passes_through(at)) {
                continue;
            }
            // Members ending here, in key order, with their directions away from the point.
            let meet: Vec<(usize, Vec3)> = lines
                .iter()
                .zip(&dirs)
                .enumerate()
                .filter_map(|(i, (l, &(a, b)))| {
                    if l.closed() {
                        None
                    } else if l.a.dist(at) < JOINT_TOL {
                        Some((i, a))
                    } else if l.b.dist(at) < JOINT_TOL {
                        Some((i, b))
                    } else {
                        None
                    }
                })
                .collect();
            // Each corner once, from its first member.
            let (Some(&(first, _)), Some(&(second, away))) = (meet.first(), meet.get(1)) else { continue };
            if first != k {
                continue;
            }
            let (Some(into), Some(away)) = (into.normalized(), away.normalized()) else { continue };
            if (-into).dot(away).clamp(-1.0, 1.0).acos() < MIN_JOINT_ANGLE {
                continue;
            }
            let (Some(a), Some(b)) = (key_of.get(first).and_then(|&i| keys.get(i)), key_of.get(second).and_then(|&i| keys.get(i))) else { continue };
            let joint = joints.iter().find(|j| j.members.contains(a) && j.members.contains(b));
            let group = groups_of.get(first).copied().unwrap_or(0).max(groups_of.get(second).copied().unwrap_or(0));
            let corner = joint.map_or_else(|| group_corner(group), |j| j.corner);
            let through = (corner == WeldCorner::Butt).then(|| joint.and_then(|j| j.through.clone()).unwrap_or_else(|| a.clone()));
            out.push(WeldCornerAt { members: [a.clone(), b.clone()], at, corner, through, own: joint.is_some() });
        }
    }
    Ok(out)
}

/// Structural Member `kind`'s members, authored in component `component`, its skeleton read from
/// `st`: each one's line, the index in [`FeatureKind::weld_keys`] of the key it goes by (its
/// first curve's) and its group.
fn skeleton(doc: &Document, st: &ModelState, kind: &FeatureKind, component: u64) -> Result<(Vec<Line>, Vec<usize>, Vec<usize>)> {
    let FeatureKind::WeldMember { path_sketch, path, groups, merge_arcs, .. } = kind else {
        return Err(DocError::Invalid("not a structural member".into()));
    };
    let keys = kind.weld_keys();
    if keys.is_empty() || keys.len() > MAX_MEMBERS {
        return Err(DocError::Invalid(format!("a structural member needs 1…{MAX_MEMBERS} skeleton lines")));
    }
    let all = std::iter::once((*path_sketch, path.as_slice())).chain(groups.iter().map(|g| (g.path_sketch, g.path.as_slice())));
    let mut lines = Vec::with_capacity(keys.len());
    let mut key_of = Vec::with_capacity(keys.len());
    let mut groups_of = Vec::with_capacity(keys.len());
    let mut at = 0;
    for (g, (sketch, path)) in all.enumerate() {
        let ps = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch} (it must come earlier in the timeline)")))?;
        let mapped = path_in_frame(doc, ps, component)?;
        let made = lines.len();
        let mut firsts = Vec::new();
        skeleton_members(mapped.as_ref(), path, keys.get(at..at + path.len()).unwrap_or_default(), *merge_arcs, &mut lines, &mut firsts)?;
        key_of.extend(firsts.iter().map(|i| at + i));
        at += path.len();
        groups_of.extend(std::iter::repeat_n(g, lines.len() - made));
    }
    Ok((lines, key_of, groups_of))
}

/// For each member of Structural Member `kind` (authored in component `component`, its skeleton
/// read from `st`), in the order they are made, the index in [`FeatureKind::weld_keys`] of the
/// key it goes by: a bent member made of several curves goes by its first.
pub fn member_keys(doc: &Document, st: &ModelState, kind: &FeatureKind, component: u64) -> Result<Vec<usize>> {
    skeleton(doc, st, kind, component).map(|(_, key_of, _)| key_of)
}

/// The one closed region a member is made from.
fn profile_region(regions: &[Region2]) -> Result<Region2> {
    match regions {
        [one] => Ok(one.clone()),
        [] => Err(DocError::Invalid("the profile sketch has no closed profile".into())),
        _ => Err(DocError::Invalid("a structural member takes one closed profile (pick one region, holes included)".into())),
    }
}

/// A member's profile as it lies on its line: mirrored about the point located on the line
/// (`anchor`), how far it reaches from there, and its outline.
struct Placed {
    region: Region2,
    anchor: Vec2,
    reach: f64,
    outline: Vec<Vec2>,
    mirror: WeldMirror,
}

fn placed(region: &Region2, locate: WeldLocate, mirror: WeldMirror) -> Result<Placed> {
    let anchor = located(region, locate)?;
    let region = match mirror {
        WeldMirror::None => region.clone(),
        WeldMirror::LeftRight => mirrored(region, |p| Vec2::new(2.0 * anchor.x - p.x, p.y)),
        WeldMirror::UpDown => mirrored(region, |p| Vec2::new(p.x, 2.0 * anchor.y - p.y)),
    };
    let reach = profile_reach(&region, anchor);
    if !(reach > 1e-6) {
        return Err(DocError::Invalid("the profile has no size".into()));
    }
    let outline = region.outer.polyline(1e-2).into_iter().collect();
    Ok(Placed { region, anchor, reach, outline, mirror })
}

/// The point of `region` that `locate` names.
fn located(region: &Region2, locate: WeldLocate) -> Result<Vec2> {
    let box_at = |u: f64, v: f64| {
        let b: solidcraft_geom::Aabb2 = region.outer.polyline(1e-4).into_iter().collect();
        Vec2::new(b.min.x + (b.max.x - b.min.x) * u, b.min.y + (b.max.y - b.min.y) * v)
    };
    Ok(match locate {
        WeldLocate::Centroid => region.centroid(),
        WeldLocate::TopLeft => box_at(0.0, 1.0),
        WeldLocate::Top => box_at(0.5, 1.0),
        WeldLocate::TopRight => box_at(1.0, 1.0),
        WeldLocate::Left => box_at(0.0, 0.5),
        WeldLocate::Middle => box_at(0.5, 0.5),
        WeldLocate::Right => box_at(1.0, 0.5),
        WeldLocate::BottomLeft => box_at(0.0, 0.0),
        WeldLocate::Bottom => box_at(0.5, 0.0),
        WeldLocate::BottomRight => box_at(1.0, 0.0),
        WeldLocate::Point([x, y]) => {
            if !(x.is_finite() && y.is_finite() && x.abs() <= crate::weld::MAX_STOCK_SIZE && y.abs() <= crate::weld::MAX_STOCK_SIZE) {
                return Err(DocError::Invalid(format!("the profile's located point must be finite and within {} mm", crate::weld::MAX_STOCK_SIZE)));
            }
            Vec2::new(x, y)
        }
    })
}

/// `region` under the mirror `f` (a reflection): every loop mapped point by point, arcs exactly,
/// then turned back the way it ran.
fn mirrored(region: &Region2, f: impl Fn(Vec2) -> Vec2) -> Region2 {
    use solidcraft_geom::Seg2;
    let map = |lp: &solidcraft_geom::Loop2| {
        let segs = lp.segs.iter().map(|s| match *s {
            Seg2::Line { a, b } => Seg2::Line { a: f(a), b: f(b) },
            Seg2::Arc { center, radius, start, sweep } => {
                // A reflection turns the start's direction to its mirror and the sweep the other way.
                let c = f(center);
                let d = f(center + Vec2::new(start.cos(), start.sin())) - c;
                Seg2::Arc { center: c, radius, start: d.y.atan2(d.x), sweep: -sweep }
            }
            Seg2::Cubic { p0, p1, p2, p3 } => Seg2::Cubic { p0: f(p0), p1: f(p1), p2: f(p2), p3: f(p3) },
            Seg2::Conic { a, apex, b, w } => Seg2::Conic { a: f(a), apex: f(apex), b: f(b), w },
            Seg2::Bezier { n, p } => Seg2::Bezier { n, p: p.map(&f) },
        });
        solidcraft_geom::Loop2 { segs: segs.collect() }.reversed()
    };
    Region2 { outer: map(&region.outer), holes: region.holes.iter().map(map).collect() }
}

/// How far the profile reaches from its anchor.
fn profile_reach(region: &Region2, anchor: Vec2) -> f64 {
    region.outer.polyline(1e-2).iter().map(|p| p.dist(anchor)).fold(0.0, f64::max)
}

/// One picked skeleton curve as path segments (world, in the feature's frame) from its own
/// start, the direction a profile's y axis leans toward along it, and whether it is free-form
/// (followed as short lines). A planar spline, conic or Bézier is free-form; so is a 3D sketch
/// curve, unless its points lie on one line or one arc (as an included model edge's do).
fn curve_segs(ps: &SolvedSketch, id: &str, key: &str) -> Result<(Vec<kernel::PathSeg>, Vec3, bool)> {
    if let Some(wi) = ps.sketch.wire_index(id) {
        let w = ps.sketch.wires.get(wi).ok_or_else(|| DocError::Unknown(format!("skeleton curve `{key}`")))?;
        let (segs, formed) = wire_segs(&w.pts);
        return Ok((segs, Vec3::Z, formed));
    }
    let ci = ps.sketch.curve_index(id).ok_or_else(|| DocError::Unknown(format!("skeleton curve `{key}`")))?;
    let (w, n) = (|p: Vec2| ps.plane.to_world(p), ps.plane.normal());
    // A spline's pieces may come as short lines already.
    let mut formed = ps.sketch.curves.get(ci).is_some_and(|c| c.kind.is_freeform());
    let mut segs = Vec::new();
    for s in ps.sketch.segs(ci) {
        match s {
            solidcraft_geom::Seg2::Line { a, b } => segs.push(kernel::PathSeg::Line { a: w(a), b: w(b) }),
            solidcraft_geom::Seg2::Arc { center, sweep, .. } => {
                segs.push(kernel::PathSeg::Arc { a: w(s.start()), center: w(center), axis: n * sweep.signum(), angle: sweep.abs() })
            }
            _ => {
                formed = true;
                let pts = thinned(&s.polyline(FORMED_TOL).into_iter().map(w).collect::<Vec<_>>());
                segs.extend(pts.windows(2).map(|q| kernel::PathSeg::Line { a: q[0], b: q[1] }));
            }
        }
    }
    if segs.len() > MAX_PATH_PIECES {
        return Err(DocError::Invalid(format!("skeleton curve `{key}` is too detailed to follow (more than {MAX_PATH_PIECES} pieces)")));
    }
    Ok((segs, n, formed))
}

/// How closely a member follows a free-form skeleton curve (mm).
const FORMED_TOL: f64 = 1e-2;

/// Most a free-form curve turns between two of its short lines (radians): more is a corner.
const FORMED_TURN: f64 = 0.35;

/// Most path pieces one member follows.
/// (The kernel's sweep takes 1000, two of them the straight runs past the ends.)
const MAX_PATH_PIECES: usize = 990;

/// A 3D sketch curve's points as the line or the arc they lie on (an included model edge's), else
/// as short lines between them, and whether it is free-form.
fn wire_segs(pts: &[Vec3]) -> (Vec<kernel::PathSeg>, bool) {
    let p = thinned(pts);
    match p.as_slice() {
        [] | [_] => (Vec::new(), false),
        [a, b] => (vec![kernel::PathSeg::Line { a: *a, b: *b }], false),
        _ => match sampled_line(&p).or_else(|| sampled_arc(&p)) {
            Some(s) => (vec![s], false),
            None => (p.windows(2).map(|q| kernel::PathSeg::Line { a: q[0], b: q[1] }).collect(), true),
        },
    }
}

/// Points along a curve without those too close to the last kept to give a direction between
/// them (the last point kept in place of the one before it).
fn thinned(pts: &[Vec3]) -> Vec<Vec3> {
    let mut p: Vec<Vec3> = Vec::with_capacity(pts.len());
    for q in pts {
        match p.last() {
            Some(l) if l.dist(*q) <= 1e-4 => {
                if p.len() > 1 {
                    p.pop();
                    p.push(*q);
                }
            }
            _ => p.push(*q),
        }
    }
    p
}

/// How far points may lie off the line or arc they sample, for its size `size` (mm).
fn sampled_tol(size: f64) -> f64 {
    1e-6 * size.max(1.0)
}

/// Points running along one line, as that line.
fn sampled_line(p: &[Vec3]) -> Option<kernel::PathSeg> {
    let (a, b) = (*p.first()?, *p.last()?);
    let d = (b - a).normalized()?;
    let tol = sampled_tol(a.dist(b));
    let on = p.iter().all(|q| q.dist_to_segment(a, b) < tol);
    let onward = p.windows(2).all(|w| (w[1] - w[0]).dot(d) > 0.0);
    (on && onward).then_some(kernel::PathSeg::Line { a, b })
}

/// Points running round one arc (or a whole circle, back to the first), as that arc.
fn sampled_arc(p: &[Vec3]) -> Option<kernel::PathSeg> {
    let n = p.len();
    let (p0, last) = (*p.first()?, *p.last()?);
    let closed = p0.dist(last) < 1e-9;
    let (p1, p2) = if closed { (*p.get(n / 3)?, *p.get(2 * n / 3)?) } else { (*p.get(n / 2)?, last) };
    let (u, v) = (p1 - p0, p2 - p0);
    let w = u.cross(v);
    let w2 = w.dot(w);
    if w2 < 1e-18 {
        return None;
    }
    let center = p0 + (v.cross(w) * u.dot(u) + w.cross(u) * v.dot(v)) * (0.5 / w2);
    let k = w.normalized()?;
    let r = p0.dist(center);
    let tol = sampled_tol(r);
    if !(r.is_finite() && p.iter().all(|q| (q.dist(center) - r).abs() < tol && (*q - center).dot(k).abs() < tol)) {
        return None;
    }
    // Round the axis the whole way, each point on from the last.
    let mut angle = 0.0;
    for w in p.windows(2) {
        let (a, b) = (w[0] - center, w[1] - center);
        let turn = k.dot(a.cross(b)).atan2(a.dot(b));
        if turn <= 0.0 {
            return None;
        }
        angle += turn;
    }
    let full = std::f64::consts::TAU;
    if closed && (angle - full).abs() > 1e-6 || angle > full + 1e-6 {
        return None;
    }
    Some(kernel::PathSeg::Arc { a: p0, center, axis: k, angle: if closed { full } else { angle } })
}

/// `path` with arcs running on about one centre and axis (as a circle's halves do) joined into one
/// arc; a ring that is all one such arc turns exactly once, so the kernel sweeps it whole.
fn joined_arcs(path: Vec<kernel::PathSeg>, ring: bool) -> Vec<kernel::PathSeg> {
    let mut out: Vec<kernel::PathSeg> = Vec::with_capacity(path.len());
    for seg in path {
        if let (Some(kernel::PathSeg::Arc { a, center, axis, angle }), kernel::PathSeg::Arc { a: a2, center: c2, axis: k2, angle: t2 }) =
            (out.last_mut(), seg)
        {
            let same = match (axis.normalized(), k2.normalized()) {
                (Some(k), Some(k2)) => k.dot(k2) > 1.0 - 1e-9 && center.dist(c2) < JOINT_TOL && (a.dist(*center) - a2.dist(c2)).abs() < JOINT_TOL,
                _ => false,
            };
            if same && *angle + t2 <= std::f64::consts::TAU + 1e-6 {
                *angle += t2;
                continue;
            }
        }
        out.push(seg);
    }
    if ring
        && let [kernel::PathSeg::Arc { angle, .. }] = out.as_mut_slice()
        && (*angle - std::f64::consts::TAU).abs() < 1e-6
    {
        *angle = std::f64::consts::TAU;
    }
    out
}

/// A group's members, added to `lines` with the index in `ids` of the curve each is keyed by, in
/// the order of those curves: each straight line a member of its own; each curved one too (an
/// arc, a free-form curve), or with `merge` each chain of curves that run on into one another
/// (tangent where they meet, no other picked curve ending there) with a curved one among them,
/// as one bent member keyed by its first curve and running the way that curve does. A curve or a
/// chain that closes on itself, tangent where it does, makes a ring, its seam where its first
/// curve starts. `keys` name the curves in errors.
fn skeleton_members(ps: &SolvedSketch, ids: &[String], keys: &[String], merge: bool, lines: &mut Vec<Line>, firsts: &mut Vec<usize>) -> Result<()> {
    // Each curve's segments, its ends and its directions there (on from its start), whether it
    // is curved and free-form, and whether it closes on itself.
    struct Curve {
        segs: Vec<kernel::PathSeg>,
        up: Vec3,
        ends: [Vec3; 2],
        dirs: [Vec3; 2],
        curved: bool,
        formed: bool,
        closed: bool,
    }
    let mut curves = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        let key = keys.get(i).map_or(id.as_str(), String::as_str);
        let (segs, up, formed) = curve_segs(ps, id, key)?;
        let no_length = || DocError::Invalid(format!("skeleton line `{key}` has no length"));
        let ends: Vec<_> = segs.iter().map(seg_ends).collect::<Option<_>>().ok_or_else(no_length)?;
        let (Some(first), Some(last)) = (ends.first(), ends.last()) else { return Err(no_length()) };
        if ends.windows(2).any(|w| w[0].1.dist(w[1].0) > JOINT_TOL || w[0].3.dot(w[1].2) < if formed { FORMED_TURN.cos() } else { TANGENT.cos() }) {
            return Err(DocError::Invalid(format!("not supported yet: skeleton curve `{key}` has a corner")));
        }
        let length: f64 = segs.iter().map(seg_length).sum();
        if !(first.0.is_finite() && last.1.is_finite() && length.is_finite()) || length < JOINT_TOL {
            return Err(no_length());
        }
        let closed = first.0.dist(last.1) < JOINT_TOL;
        if closed && last.3.dot(first.2) < TANGENT.cos() {
            return Err(DocError::Invalid(format!(
                "not supported yet: skeleton curve `{key}` closes on itself at a corner (a ring must run on smoothly)"
            )));
        }
        let curved = formed || segs.iter().any(|s| matches!(s, kernel::PathSeg::Arc { .. }));
        curves.push(Curve { ends: [first.0, last.1], dirs: [first.2, last.3], segs, up, curved, formed, closed });
    }
    // The curve (and its end) each curve's end runs on into, where they merge.
    let mut next: Vec<[Option<(usize, usize)>; 2]> = vec![[None, None]; curves.len()];
    if merge {
        for (i, c) in curves.iter().enumerate().filter(|(_, c)| !c.closed) {
            for (e, &p) in c.ends.iter().enumerate() {
                let mut there = curves
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .flat_map(|(j, o)| (0..2).filter(move |&f| o.ends[f].dist(p) < JOINT_TOL).map(move |f| (j, f)));
                let (Some((j, f)), None) = (there.next(), there.next()) else { continue };
                let Some(o) = curves.get(j).filter(|o| !o.closed) else { continue };
                // On out of this curve, and on into the other.
                let out = if e == 0 { -c.dirs[0] } else { c.dirs[1] };
                let on = if f == 0 { o.dirs[0] } else { -o.dirs[1] };
                if (c.curved || o.curved)
                    && out.dot(on) >= TANGENT.cos()
                    && let Some(slot) = next.get_mut(i).and_then(|n| n.get_mut(e))
                {
                    *slot = Some((j, f));
                }
            }
        }
    }
    let key_of = |i: usize| keys.get(i).or_else(|| ids.get(i)).cloned().unwrap_or_default();
    let mut used = vec![false; curves.len()];
    for i in 0..curves.len() {
        if used.get(i).copied().unwrap_or(true) {
            continue;
        }
        // Back from curve `i` to an end of its chain (or round to `i` again: a ring).
        let (mut at, mut back) = (i, 0);
        let mut steps = 0;
        let mut ring = curves.get(i).is_some_and(|c| c.closed);
        while let Some((j, f)) = next.get(at).and_then(|n| n.get(back).copied().flatten()) {
            (at, back) = (j, 1 - f);
            steps += 1;
            if at == i {
                ring = true;
                break;
            }
            if steps > curves.len() {
                return Err(DocError::Invalid(format!("the skeleton curves through `{}` don't make a chain", key_of(i))));
            }
        }
        let mut chain = Vec::new();
        if ring {
            // Round from curve `i`, the way it runs.
            chain.push((i, false));
            let (mut at, mut out) = (i, 1);
            while let Some((j, f)) = next.get(at).and_then(|n| n.get(out).copied().flatten()) {
                if j == i || chain.len() > curves.len() {
                    break;
                }
                chain.push((j, f == 1));
                (at, out) = (j, 1 - f);
            }
        } else {
            // `back` is the free end of `at`: the chain runs out of its other end.
            chain.push((at, back == 1));
            let mut out = 1 - back;
            while let Some((j, f)) = next.get(at).and_then(|n| n.get(out).copied().flatten()) {
                chain.push((j, f == 1));
                (at, out) = (j, 1 - f);
                if chain.len() > curves.len() {
                    break;
                }
            }
            // Run the way the chain's first curve runs.
            if chain.iter().any(|&(c, reversed)| c == i && reversed) {
                chain.reverse();
                chain.iter_mut().for_each(|c| c.1 = !c.1);
            }
        }
        let mut path = Vec::new();
        let mut formed = false;
        for &(c, reversed) in &chain {
            if let Some(u) = used.get_mut(c) {
                *u = true;
            }
            let Some(curve) = curves.get(c) else { continue };
            formed |= curve.formed;
            if reversed {
                path.extend(curve.segs.iter().rev().map(seg_reversed));
            } else {
                path.extend(curve.segs.iter().copied());
            }
        }
        let path = joined_arcs(path, ring);
        if path.len() > MAX_PATH_PIECES {
            return Err(DocError::Invalid(format!(
                "the member through `{}` is too detailed to follow (more than {MAX_PATH_PIECES} pieces)",
                key_of(i)
            )));
        }
        let (Some(first), Some(last)) = (path.first().and_then(seg_ends), path.last().and_then(seg_ends)) else { continue };
        let up = curves.get(i).map_or(Vec3::Z, |c| c.up);
        let line = match path.as_slice() {
            [kernel::PathSeg::Line { a, b }] => Line::straight(*a, *b, up),
            _ => Line { a: first.0, b: if ring { first.0 } else { last.1 }, up, path, formed },
        };
        if line.path.is_empty()
            && lines.iter().any(|o: &Line| {
                o.path.is_empty()
                    && ((o.a.dist(line.a) < JOINT_TOL && o.b.dist(line.b) < JOINT_TOL)
                        || (o.a.dist(line.b) < JOINT_TOL && o.b.dist(line.a) < JOINT_TOL))
            })
        {
            return Err(DocError::Invalid(format!("skeleton line `{}` lies on another member", key_of(i))));
        }
        lines.push(line);
        firsts.push(i);
    }
    Ok(())
}

/// The cut at one end of member `k` (its start `a`, else its end `b`):
/// - where another member's line passes through, a butt against that member's face;
/// - where one other member ends, the feature's corner (or the joint's own): a mitre on the plane
///   halfway between them, or a butt, the member running through cut square past the other;
/// - where more members end, the first two listed make the corner and the others butt against
///   the first;
/// - an end on a member of an earlier feature (through it, or alone at its end), a butt against
///   that member's face; one through it is butted even where this feature makes a corner, as
///   the earlier member fills the corner;
/// - a free end, square.
///
/// A bent member's ends are joined along its direction there; an end on a bent member (its
/// straight run or its bend, or anywhere round a ring) butts against its face there, flat on the
/// plane touching it. A ring's seam is cut square.
fn end_cut(members: &[Member], rules: &Rules, k: usize, start: bool, notes: &mut Vec<String>) -> EndCut {
    let Some(me) = members.get(k) else { return EndCut { at: Vec3::ZERO, normal: Vec3::Z } };
    let (at, into) = me.end(start);
    let square = EndCut { at, normal: into };
    if me.line.closed() {
        return square;
    }
    if let Some(t) = members.iter().position(|m| m.passes_through(at)) {
        return butt(members, rules, k, t, at, into);
    }
    if let Some(cut) = butt_earlier(rules, at, into, |m| m.passes_through(at)) {
        return cut;
    }
    // The members ending here, in the path's order, with their directions away from the joint.
    let meet: Vec<(usize, Vec3)> = members
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            if m.line.closed() {
                None
            } else if m.line.a.dist(at) < JOINT_TOL {
                Some((i, m.end(true).1))
            } else if m.line.b.dist(at) < JOINT_TOL {
                Some((i, m.end(false).1))
            } else {
                None
            }
        })
        .collect();
    let (Some(&(first, _)), Some(&(second, _))) = (meet.first(), meet.get(1)) else {
        return butt_earlier(rules, at, into, |m| m.ends_at(at)).unwrap_or(square);
    };
    let other = if k == first {
        second
    } else if k == second {
        first
    } else {
        return butt(members, rules, k, first, at, into);
    };
    let Some(&(_, away)) = meet.iter().find(|(i, _)| *i == other) else { return square };
    let angle = into.dot(away).clamp(-1.0, 1.0).acos();
    if angle < MIN_JOINT_ANGLE {
        notes.push(format!("members meeting at {} fold back on each other: their ends are cut square", show_point(at)));
        return square;
    }
    let (Some(a), Some(b)) = (members.get(k), members.get(other)) else { return square };
    let joint = rules.joints.iter().find(|j| j.members.iter().any(|m| m == a.id) && j.members.iter().any(|m| m == b.id));
    // Members of two groups meet as the later group joins them.
    let group_corner = rules.corners.get(a.group.max(b.group)).copied().unwrap_or_default();
    match joint.map_or(group_corner, |j| j.corner) {
        // The mitre plane bisects the two members: its normal is the difference of their
        // directions away from the joint, which keeps this member on its positive side.
        WeldCorner::Mitre => (into - away).normalized().map_or(square, |normal| EndCut { at, normal }),
        WeldCorner::Butt => {
            let through = match joint.and_then(|j| j.through.as_deref()) {
                Some(t) if t == a.id => k,
                Some(_) => other,
                None => k.min(other),
            };
            if through == k {
                // Square, past the other member's far side.
                EndCut { at: at - into * extent_of(b.outline_at(at), -into), normal: into }
            } else {
                butt(members, rules, k, through, at, into)
            }
        }
    }
}

/// Member `k`'s end at `at` (running `into` the member) butted against the face of member `t`
/// toward it, the gap away; square where the two run on in one line.
fn butt(members: &[Member], rules: &Rules, k: usize, t: usize, at: Vec3, into: Vec3) -> EndCut {
    let (Some(_), Some(t)) = (members.get(k), members.get(t)) else { return EndCut { at, normal: into } };
    butt_against(t, rules.gap, at, into)
}

/// An end at `at` butted against the first earlier member `meets` that it doesn't run on in line
/// with.
fn butt_earlier(rules: &Rules, at: Vec3, into: Vec3, meets: impl Fn(&Member) -> bool) -> Option<EndCut> {
    rules.earlier.iter().filter(|m| meets(m)).map(|t| butt_against(t, rules.gap, at, into)).find(|c| c.at.dist(at) > 0.0 || c.normal.dist(into) > 0.0)
}

/// An end at `at` (running `into` its member) butted against the face of member `t` toward it,
/// `gap` away; square where the two run on in one line.
fn butt_against(t: &Member, gap: f64, at: Vec3, into: Vec3) -> EndCut {
    let Some((dir, outline)) = t.across(at) else { return EndCut { at, normal: into } };
    match (into - dir * into.dot(dir)).normalized() {
        Some(n) if n.dot(into) > MIN_JOINT_ANGLE.sin() => EndCut { at: at + n * (extent_of(&outline, n) + gap), normal: n },
        _ => EndCut { at, normal: into },
    }
}

fn show_point(p: Vec3) -> String {
    format!("({}, {}, {})", show_mm(p.x), show_mm(p.y), show_mm(p.z))
}

/// The profile's frame at the start of `line`: its normal along the line, its y axis toward
/// `line.up` (another axis when the line runs along it), turned by `turn` about the line, and
/// placed so the profile's `anchor` lies on the line.
fn member_frame(line: &Line, anchor: Vec2, turn: f64) -> Result<Plane> {
    let d = line.tangents().ok_or_else(|| DocError::Invalid("skeleton line direction".into()))?.0;
    let lean = |v: Vec3| (v - d * v.dot(d)).normalized();
    let y = lean(line.up).filter(|_| d.dot(line.up).abs() < 0.999).or_else(|| lean(Vec3::X)).or_else(|| lean(Vec3::Y));
    let y = y.ok_or_else(|| DocError::Invalid("profile frame for a skeleton line".into()))?;
    let x = y.cross(d);
    let (sin, cos) = turn.sin_cos();
    let (x, y) = (x * cos + y * sin, y * cos - x * sin);
    let origin = line.a - x * anchor.x - y * anchor.y;
    Plane::new(origin, x, y).ok_or_else(|| DocError::Invalid("profile frame for a skeleton line".into()))
}

/// Members recently built, by everything they are made from: picking one more line in the
/// dialog rebuilds only the members whose cuts it changes, and OK reuses what the preview built.
static MEMBERS: std::sync::Mutex<Vec<(u64, Body)>> = std::sync::Mutex::new(Vec::new());
const MEMBERS_KEPT: usize = 256;

/// One member: the profile extruded past both ends of its line, then cut at each end.
fn member(line: &Line, region: &Region2, anchor: Vec2, turn: f64, reach: f64, start: EndCut, end: EndCut) -> Result<Body> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{line:?}|{region:?}|{anchor:?}|{turn:?}|{reach:?}|{start:?}|{end:?}").hash(&mut h);
    let key = h.finish();
    if let Ok(mut kept) = MEMBERS.lock()
        && let Some(i) = kept.iter().position(|(k, _)| *k == key)
    {
        let hit = kept.remove(i);
        let body = hit.1.clone();
        kept.insert(0, hit);
        return Ok(body);
    }
    let body = member_uncached(line, region, anchor, turn, reach, start, end)?;
    if let Ok(mut kept) = MEMBERS.lock() {
        kept.insert(0, (key, body.clone()));
        kept.truncate(MEMBERS_KEPT);
    }
    Ok(body)
}

fn member_uncached(line: &Line, region: &Region2, anchor: Vec2, turn: f64, reach: f64, start: EndCut, end: EndCut) -> Result<Body> {
    let frame = member_frame(line, anchor, turn)?;
    if !line.path.is_empty() {
        return bent_member(line, region, &frame, reach, start, end);
    }
    let length = line.a.dist(line.b);
    let d = frame.normal();
    // A cut plane leaning from the square by θ reaches reach·tan θ along the member.
    let overrun = |c: &EndCut| {
        let along = c.normal.dot(d).abs().max(MIN_JOINT_ANGLE.sin());
        reach * (1.0 - along * along).sqrt() / along + reach + 1.0
    };
    // A square end is where the extrusion stops: only a leaning one is cut.
    let from = square_at(&start, frame.origin, d).unwrap_or(-overrun(&start));
    let to = square_at(&end, frame.origin, d).unwrap_or(length + overrun(&end));
    let mut bodies = kernel::extrude(&frame, std::slice::from_ref(region), from, to)?;
    if bodies.len() != 1 {
        return Err(DocError::Invalid("the profile made more than one solid".into()));
    }
    let body = bodies.pop().ok_or_else(|| DocError::Invalid("the profile made no solid".into()))?;
    let leaning: Vec<EndCut> = [start, end].into_iter().filter(|c| square_at(c, frame.origin, d).is_none()).collect();
    cut_ends(body, &frame, region, (from, to), &leaning)
}

/// How far a cut plane leaning from square to a member's direction `d` reaches along it, the
/// profile reaching `reach` from the line: reach·tan θ.
fn lean_reach(c: &EndCut, d: Vec3, reach: f64) -> f64 {
    let along = c.normal.dot(d).abs().max(MIN_JOINT_ANGLE.sin());
    reach * (1.0 - along * along).sqrt() / along
}

/// A bent member: the profile (in `frame` at the line's start, reaching `reach` from the line)
/// swept along the line's centreline, run on straight past each end as far as its cut reaches,
/// and cut there. Each cut takes away only what lies near its end, so a plane through one end
/// leaves the rest of a tightly bent member alone. A ring is swept the whole way round, uncut.
fn bent_member(line: &Line, region: &Region2, frame: &Plane, reach: f64, start: EndCut, end: EndCut) -> Result<Body> {
    if line.closed() {
        return Ok(kernel::sweep(frame, region, &line.path)?);
    }
    let (t0, t1) = line.tangents().ok_or_else(|| DocError::Invalid("skeleton line direction".into()))?;
    // Each end: its point, its direction out of the member, its cut, how far the cut lies past the
    // point, whether it is square, and how far the path runs on past the point.
    let ends = [(line.a, -t0, start), (line.b, t1, end)].map(|(p, out, c)| {
        let past = (c.at - p).dot(out);
        let square = c.normal.dot(-out) > 1.0 - 1e-12;
        let on = if square { past.max(0.0) } else { (past + lean_reach(&c, out, reach) + reach + 1.0).max(0.0) };
        (p, out, c, past, square, on)
    });
    let [(_, _, _, _, _, on0), (_, _, _, _, _, on1)] = ends;
    let mut path = Vec::with_capacity(line.path.len() + 2);
    if on0 > 0.0 {
        path.push(kernel::PathSeg::Line { a: line.a - t0 * on0, b: line.a });
    }
    path.extend(line.path.iter().copied());
    if on1 > 0.0 {
        path.push(kernel::PathSeg::Line { a: line.b, b: line.b + t1 * on1 });
    }
    let mut at = *frame;
    at.origin -= t0 * on0;
    let mut body = kernel::sweep(&at, region, &path)?;
    for (p, out, c, past, square, on) in ends {
        // A square end the path stops at needs no cut.
        if square && past >= 0.0 {
            continue;
        }
        body = cut_near_end(&body, p, out, &c, reach, on)?;
    }
    Ok(body)
}

/// `body` cut on `c` near a member's end `p` (running `out` of the member there, its profile
/// reaching `reach` from the line, its stock running on `on` past `p`): only what lies within a
/// block round that end is taken away.
fn cut_near_end(body: &Body, p: Vec3, out: Vec3, c: &EndCut, reach: f64, on: f64) -> Result<Body> {
    let side = Plane::from_normal(p, out).ok_or_else(|| DocError::Invalid("member end plane".into()))?;
    let room = reach + 1.0;
    let past = (c.at - p).dot(out);
    let inward = (-past).max(0.0) + lean_reach(c, out, reach) + room;
    let mut hs = Vec::with_capacity(7);
    for n in [side.x, side.y] {
        hs.push(kernel::HalfSpace { n, d: n.dot(p) + room });
        hs.push(kernel::HalfSpace { n: -n, d: -n.dot(p) + room });
    }
    hs.push(kernel::HalfSpace { n: out, d: out.dot(p) + on.max(0.0) + room });
    hs.push(kernel::HalfSpace { n: -out, d: -out.dot(p) + inward });
    // What the cut takes away: the side its normal points away from.
    hs.push(kernel::HalfSpace { n: c.normal, d: c.normal.dot(c.at) });
    let tool = kernel::convex_polyhedron(&hs)?;
    kernel::boolean(body, &tool, BoolOp::Cut)?.ok_or_else(|| DocError::Invalid("a member end cut left nothing".into()))
}

/// A bent member's record: its centreline, its bends (each where it starts, its angle and radius
/// and the way it bends in the profile's frame there), its length past each end of its
/// centreline, cut on `start` and `end` (and coped by `copes`), and its profile's frame at its end
/// (`anchor` on its line). `None` for a straight member.
fn bent_record(
    m: &Member,
    start: &EndCut,
    end: &EndCut,
    copes: &[Option<crate::weld::Cope>; 2],
    anchor: Vec2,
    allowance: f64,
) -> Option<crate::weld::Bent> {
    if m.line.path.is_empty() {
        return None;
    }
    let ((mut x, mut y), mut t) = (m.axes, m.dir);
    let mut done = 0.0;
    let mut path = Vec::with_capacity(m.line.path.len());
    let mut bends: Vec<crate::weld::Bend> = Vec::new();
    for seg in &m.line.path {
        // The profile's axes carried on as `carry` carries them.
        let (_, _, d0, d1) = seg_ends(seg)?;
        (x, y) = (aligned(x, t, d0), aligned(y, t, d0));
        t = d1;
        match *seg {
            kernel::PathSeg::Line { a, b } => path.push(crate::weld::PathPiece::Line { a, b }),
            kernel::PathSeg::Arc { a, center, axis, angle } => {
                path.push(crate::weld::PathPiece::Arc { a, center, axis, angle });
                let k = axis.normalized()?;
                let toward = (center - a).normalized()?;
                let radius = a.dist(center);
                // Arcs of one radius and plane running on into each other are one bend.
                match bends.last_mut() {
                    Some(last)
                        if (last.at + last.length() - done).abs() < JOINT_TOL && (last.radius - radius).abs() < JOINT_TOL && {
                            let [u, v] = last.toward;
                            let t = [toward.dot(x), toward.dot(y)];
                            (u - t[0]).abs() < 1e-6 && (v - t[1]).abs() < 1e-6
                        } =>
                    {
                        last.angle += angle.to_degrees();
                    }
                    _ => bends.push(crate::weld::Bend { at: done, angle: angle.to_degrees(), radius, toward: [toward.dot(x), toward.dot(y)] }),
                }
                (x, y) = (turned(x, k, angle), turned(y, k, angle));
            }
        }
        done += seg_length(seg);
    }
    let (xe, ye) = m.end_axes;
    let end_frame = Plane::new(m.line.b - xe * anchor.x - ye * anchor.y, xe, ye)?;
    Some(crate::weld::Bent { path, bends, developed: m.line.length(), lead: m.leads(start, end, copes), end_frame, allowance, formed: m.line.formed })
}

/// `body`, `region` extruded in `frame` over `range` along its normal, cut on the planes of
/// `cuts` (keeping the side each normal points to). Two cuts are made at once, by one boolean
/// with the slab between their planes: a member's booleans cost the most, most of all on round
/// profiles (~0.2 s a cut on a tube).
fn cut_ends(mut body: Body, frame: &Plane, region: &Region2, range: (f64, f64), cuts: &[EndCut]) -> Result<Body> {
    if let [a, b] = cuts
        && let Some(slab) = slab(frame, region, range, [a, b])
        && let Ok(Some(cut)) = kernel::boolean(&body, &slab, BoolOp::Intersect)
    {
        return Ok(cut);
    }
    // One cut, or the slab failed: plane by plane.
    for cut in cuts {
        let plane = Plane::from_normal(cut.at, cut.normal).ok_or_else(|| DocError::Invalid("member end plane".into()))?;
        body = kernel::half_space(&body, &plane, true)?.0.ok_or_else(|| DocError::Invalid("a member end cut left nothing".into()))?;
    }
    Ok(body)
}

/// The convex block between two end cut planes, around `region` extruded over `range` in
/// `frame` with room to spare: what is left of that extrusion cut on both planes is its
/// intersection with the block.
fn slab(frame: &Plane, region: &Region2, range: (f64, f64), cuts: [&EndCut; 2]) -> Option<Body> {
    let pts = region.outer.polyline(1e-2);
    let lo = pts.iter().fold(Vec2::new(f64::INFINITY, f64::INFINITY), |m, p| Vec2::new(m.x.min(p.x), m.y.min(p.y)));
    let hi = pts.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |m, p| Vec2::new(m.x.max(p.x), m.y.max(p.y)));
    let (lo, hi) = ((lo.x, lo.y, range.0.min(range.1)), (hi.x, hi.y, range.0.max(range.1)));
    let room = 1.0 + 0.01 * ((hi.0 - lo.0) + (hi.1 - lo.1) + (hi.2 - lo.2));
    let d = frame.normal();
    let o = frame.origin;
    // `n · x <= c`, `n` outward: each axis both ways, then each cut plane (the kept side is the
    // one its normal points to).
    let mut hs = Vec::with_capacity(8);
    for (n, a, b) in [(frame.x, lo.0, hi.0), (frame.y, lo.1, hi.1), (d, lo.2, hi.2)] {
        hs.push(kernel::HalfSpace { n, d: n.dot(o) + b + room });
        hs.push(kernel::HalfSpace { n: -n, d: -(n.dot(o) + a - room) });
    }
    for c in cuts {
        hs.push(kernel::HalfSpace { n: -c.normal, d: -c.normal.dot(c.at) });
    }
    (lo.0.is_finite() && hs.iter().all(|h| h.n.is_finite() && h.d.is_finite())).then(|| kernel::convex_polyhedron(&hs).ok()).flatten()
}

/// Where an end cut square to the member (direction `d`) lies along it from `origin`; `None` for
/// a leaning cut.
fn square_at(c: &EndCut, origin: Vec3, d: Vec3) -> Option<f64> {
    (c.normal.dot(d).abs() > 1.0 - 1e-12).then(|| (c.at - origin).dot(d))
}

/// A member as later features see it from its record: its line (a bent member's centreline),
/// profile axes and outline. `None` for a record without its outline, or a bent one without its
/// stock.
fn recorded(w: &crate::weld::MemberCut) -> Option<Member<'_>> {
    if w.outline.is_empty() {
        return None;
    }
    let [a, b] = w.axis;
    let axes = w.stock.as_ref().map_or((Vec3::ZERO, Vec3::ZERO), |s| (s.frame.x, s.frame.y));
    let Some(bent) = &w.bent else {
        let dir = (b - a).normalized()?;
        return Some(Member::straight(&w.body, 0, Line::straight(a, b, Vec3::Z), dir, axes, w.outline.clone()));
    };
    w.stock.as_ref()?;
    let path = bent
        .path
        .iter()
        .map(|p| match *p {
            crate::weld::PathPiece::Line { a, b } => kernel::PathSeg::Line { a, b },
            crate::weld::PathPiece::Arc { a, center, axis, angle } => kernel::PathSeg::Arc { a, center, axis, angle },
        })
        .collect();
    let profile = w.outline.iter().map(|o| Vec2::new(o.dot(axes.0), o.dot(axes.1))).collect();
    Some(Member::carried(&w.body, 0, Line { a, b, up: Vec3::Z, path, formed: bent.formed }, axes, profile))
}

/// The end of member `m` nearest `plane` (0 its start, 1 its end), cut on the plane `gap` toward
/// the member.
fn plane_end(m: &Member, plane: &Plane, gap: f64) -> (usize, EndCut) {
    let n = plane.normal();
    let dist = |p: Vec3| n.dot(p - plane.origin).abs();
    let end = usize::from(dist(m.line.b) < dist(m.line.a));
    let into = m.end(end == 0).1;
    let normal = if n.dot(into) < 0.0 { -n } else { n };
    (end, EndCut { at: plane.origin + normal * gap, normal })
}

/// The end of member `m` nearest one of `targets` (by its centreline): that target and the end.
fn member_end(m: &Member, targets: &[Member]) -> Option<(usize, usize)> {
    let mut best: Option<(f64, usize, usize)> = None;
    for (k, t) in targets.iter().enumerate() {
        for (end, p) in [m.line.a, m.line.b].into_iter().enumerate() {
            let d = t.line.nearest(p).dist(p);
            if best.is_none_or(|(bd, ..)| d < bd) {
                best = Some((d, k, end));
            }
        }
    }
    best.map(|(_, k, end)| (k, end))
}

/// A member made again from its stock, cut on its two end planes and coped where an end is.
fn cut_member(stock: &crate::weld::Stock, cuts: &[EndCut; 2], copes: &[Option<crate::weld::Cope>; 2]) -> Result<Body> {
    let frame = &stock.frame;
    let d = frame.normal();
    // How far the profile reaches from the frame's origin, and so past a leaning cut.
    let reach = stock.region.outer.polyline(1e-2).iter().map(|p| p.len()).fold(0.0, f64::max);
    let mut range = (f64::INFINITY, f64::NEG_INFINITY);
    for c in cuts {
        let along = c.normal.dot(d);
        if along.abs() < MIN_JOINT_ANGLE.sin() {
            return Err(DocError::Invalid("a member end cut runs along the member".into()));
        }
        let t = c.normal.dot(c.at - frame.origin) / along;
        let lean = reach * (1.0 - along * along).max(0.0).sqrt() / along.abs() + reach + 1.0;
        range = (range.0.min(t - lean), range.1.max(t + lean));
    }
    // A square end is where the extrusion stops: only a leaning one is cut.
    let square: Vec<(f64, f64)> = cuts.iter().filter_map(|c| square_at(c, frame.origin, d).map(|t| (t, c.normal.dot(d)))).collect();
    for (t, along) in &square {
        if *along > 0.0 { range.0 = *t } else { range.1 = *t }
    }
    let mut bodies = kernel::extrude(frame, std::slice::from_ref(&stock.region), range.0, range.1)?;
    if bodies.len() != 1 {
        return Err(DocError::Invalid("the profile made more than one solid".into()));
    }
    let body = bodies.pop().ok_or_else(|| DocError::Invalid("the profile made no solid".into()))?;
    let leaning: Vec<EndCut> = cuts.iter().copied().filter(|c| square_at(c, frame.origin, d).is_none()).collect();
    let mut body = cut_ends(body, frame, &stock.region, range, &leaning)?;
    for c in copes.iter().flatten() {
        body = cope(&body, c)?;
    }
    Ok(body)
}

/// `body` with the outside of the member `c` describes taken away.
fn cope(body: &Body, c: &crate::weld::Cope) -> Result<Body> {
    let mut tools = kernel::extrude(&c.frame, std::slice::from_ref(&c.region), c.from, c.to)?;
    let tool =
        tools.pop().filter(|_| tools.is_empty()).ok_or_else(|| DocError::Invalid("the coped member's outline made more than one solid".into()))?;
    kernel::boolean(body, &tool, BoolOp::Cut)?.ok_or_else(|| DocError::Invalid("the cope leaves nothing of the member".into()))
}

/// The outside of member `t` (`tm` as later features see it: its profile's outer outline along
/// the straight piece of its line through `q`), `gap` larger toward `n`, to cope another member's
/// end to. `None` where `t` has no stock, or bends at `q`.
fn cope_of(t: &crate::weld::MemberCut, tm: &Member, q: Vec3, n: Vec3, gap: f64) -> Option<crate::weld::Cope> {
    let stock = t.stock.as_ref()?;
    let region = Region2 { outer: stock.region.outer.clone(), holes: Vec::new() };
    if tm.line.path.is_empty() {
        let d = stock.frame.normal();
        let mut frame = stock.frame;
        frame.origin += n * gap;
        let [a, b] = t.axis;
        let (from, to) = (d.dot(a - stock.frame.origin), d.dot(b - stock.frame.origin));
        return Some(crate::weld::Cope { region, frame, from: from.min(to), to: from.max(to) });
    }
    if tm.line.formed {
        return None;
    }
    // The straight run through `q`, and the stock's frame carried onto it (its located point on
    // the line).
    let (a, b) = tm.line.path.iter().find_map(|s| match *s {
        kernel::PathSeg::Line { a, b } if q.dist_to_segment(a, b) < JOINT_TOL => Some((a, b)),
        _ => None,
    })?;
    let (d, (x, y)) = carry(&tm.line, tm.axes, Some(a.lerp(b, 0.5)))?;
    let o = tm.line.a - stock.frame.origin;
    let anchor = Vec2::new(stock.frame.x.dot(o), stock.frame.y.dot(o));
    let frame = Plane::new(a - x * anchor.x - y * anchor.y + n * gap, x, y)?;
    let (from, to) = (d.dot(a - frame.origin), d.dot(b - frame.origin));
    Some(crate::weld::Cope { region, frame, from: from.min(to), to: from.max(to) })
}

/// What a Trim/Extend cuts its members' ends to.
enum Boundary {
    /// Flat on a plane.
    Plane(Plane),
    /// Coped to the outside of a round face.
    Round(kernel::CylinderFace),
    /// Against (or coped to) the nearest of the `to` members.
    Members,
}

/// Face `i` of `body` (picked at `at`) as a cylinder whose outside a member can be coped to.
fn round_face(body: &ModelBody, i: usize, at: Vec3) -> Result<kernel::CylinderFace> {
    let curved = |what: &str| DocError::Invalid(format!("not supported yet: coping to {what}"));
    let c = match kernel::face_surface(&body.body, i) {
        Some(kernel::FaceSurface::Cylinder { .. }) => kernel::cylinder_face_at(&body.body, at).ok_or_else(|| curved("this round face"))?,
        Some(kernel::FaceSurface::Sphere { .. }) => return Err(curved("a sphere")),
        Some(kernel::FaceSurface::Cone { .. }) => return Err(curved("a cone")),
        Some(kernel::FaceSurface::Torus { .. }) => return Err(curved("a torus")),
        _ => return Err(curved("a freeform face")),
    };
    if c.internal {
        return Err(DocError::Invalid("the face is the inside of a hole: a cope fits the outside of a round face".into()));
    }
    if !(c.radius.is_finite() && c.radius > 1e-6 && c.start.is_finite() && c.end.is_finite() && c.end > c.start) {
        return Err(curved("this round face"));
    }
    Ok(c)
}

/// The end of member `m` nearest the axis of round face `c`, cut through the axis and coped to
/// the face's cylinder (its radius `gap` larger, along the face's length); `None` when the member
/// runs along the axis or passes beside the face.
fn round_end(m: &Member, c: &kernel::CylinderFace, gap: f64) -> Option<(usize, EndCut, crate::weld::Cope)> {
    let axis: Vec3 = c.axis.into();
    let off = |p: Vec3| {
        let v = p - c.axis_point;
        (v - axis * v.dot(axis)).len()
    };
    let end = usize::from(off(m.line.b) < off(m.line.a));
    let (p, into) = m.end(end == 0);
    let normal = (into - axis * into.dot(axis)).normalized().filter(|n| n.dot(into) > MIN_JOINT_ANGLE.sin())?;
    // Where the member's line passes the axis, along the axis: within the face, or the profile
    // reaching onto it.
    let s = (p - c.axis_point).dot(axis) - (p - c.axis_point).dot(normal) * into.dot(axis) / into.dot(normal);
    let reach = m.outline_at(p).iter().map(|o| o.len()).fold(0.0, f64::max);
    if s < c.start - reach || s > c.end + reach {
        return None;
    }
    let frame = Plane::from_normal(c.axis_point, axis)?;
    let region = Region2 { outer: solidcraft_geom::Loop2::circle(Vec2::new(0.0, 0.0), c.radius + gap), holes: Vec::new() };
    Some((end, EndCut { at: c.axis_point, normal }, crate::weld::Cope { region, frame, from: c.start, to: c.end }))
}

/// Longest a Trim/Extend makes a member (mm).
const MAX_TRIM_LENGTH: f64 = 1.0e6;

/// `line` with its end `end` (0 its start) moved `s` along its direction out of the member there
/// (back where negative): a straight line's end along the line, a bent member's along its last
/// straight run (or along a straight run added past its last bend). `None` where a bent member
/// would be cut back into a bend.
fn moved_end(line: &Line, end: usize, s: f64) -> Option<Line> {
    let mut l = line.clone();
    let (t0, t1) = l.tangents()?;
    if l.path.is_empty() {
        if end == 0 {
            l.a -= t0 * s
        } else {
            l.b += t1 * s
        }
        return Some(l);
    }
    if end == 0 {
        let p = l.a - t0 * s;
        match l.path.first_mut() {
            Some(kernel::PathSeg::Line { a, b }) if a.dist(*b) + s > JOINT_TOL => *a = p,
            Some(_) if s > 0.0 => l.path.insert(0, kernel::PathSeg::Line { a: p, b: l.a }),
            _ => return None,
        }
        l.a = p;
    } else {
        let p = l.b + t1 * s;
        match l.path.last_mut() {
            Some(kernel::PathSeg::Line { a, b }) if a.dist(*b) + s > JOINT_TOL => *b = p,
            Some(_) if s > 0.0 => l.path.push(kernel::PathSeg::Line { a: l.b, b: p }),
            _ => return None,
        }
        l.b = p;
    }
    Some(l)
}

/// Evaluation of a weldment Trim/Extend: each member's end nearest the boundary moved onto it. A
/// member no later feature has changed is made again from its stock with that end; a changed one
/// can only be cut back (by the boundary's plane). The member's record (its line, length and end
/// cuts) follows, for the cut list and later Structural Members. A bent member's end moves along
/// its straight run there; the boundary members may be bent (an end meeting one on its bend
/// butts flat against the plane touching it there, and is coped only to a straight run).
pub(super) fn trim(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState, warning: &mut Option<String>) -> Result<()> {
    let FeatureKind::WeldTrim { members, plane, face, to, mode, gap, cope: coped } = &f.kind else {
        return Err(DocError::Invalid(format!("{} is not a trim/extend", f.name)));
    };
    if members.is_empty() || members.len() > MAX_MEMBERS || to.len() > MAX_MEMBERS {
        return Err(DocError::Invalid(format!("a trim/extend takes 1…{MAX_MEMBERS} members")));
    }
    let gap = match gap {
        Some(g) => val(vals, g, Kind::Length)?,
        None => 0.0,
    };
    if !(gap.is_finite() && gap >= 0.0) {
        return Err(DocError::Invalid("the gap must be zero or more".into()));
    }
    let record = |st: &ModelState, name: &str| {
        st.welds
            .iter()
            .position(|w| w.body == name && st.body(name).is_some())
            .ok_or_else(|| DocError::Invalid(format!("`{name}` is not a structural member")))
    };
    let targets: Vec<crate::weld::MemberCut> = to
        .iter()
        .map(|b| record(st, &b.0).and_then(|i| st.welds.get(i).cloned().ok_or_else(|| DocError::Unknown(b.0.clone()))))
        .collect::<Result<_>>()?;
    let target_members: Vec<Member> = targets.iter().filter(|t| t.stock.is_some()).filter_map(recorded).collect();
    if target_members.len() != targets.len() {
        return Err(DocError::Invalid("a boundary member has no stock to cut against".into()));
    }
    if *coped && to.is_empty() && face.is_none() {
        return Err(DocError::Invalid("a cope needs members or a round face as the boundary".into()));
    }
    let boundary = if !to.is_empty() {
        Boundary::Members
    } else if let Some(fa) = face {
        if members.iter().any(|m| m.0 == fa.body.0) {
            return Err(DocError::Invalid(format!("`{}` is trimmed: its own face can't be the boundary", fa.body.0)));
        }
        let tb = st.body(&fa.body.0).ok_or_else(|| DocError::Unknown(format!("body `{}`", fa.body.0)))?;
        let i = crate::appearance::face_index_at(tb, fa.point.0)
            .ok_or_else(|| DocError::Invalid(format!("no face of `{}` at the boundary point", fa.body.0)))?;
        match tb.body.face_plane(i).or_else(|| tb.body.planar_face(i)) {
            Some(p) => Boundary::Plane(p),
            None if *coped => Boundary::Round(round_face(tb, i, fa.point.0)?),
            None => return Err(DocError::Invalid("not supported yet: cutting flat to a curved face (a round face can be coped to)".into())),
        }
    } else if let Some(p) = plane {
        Boundary::Plane(doc.resolve_plane_in(vals, p, 0, Some(st))?)
    } else {
        return Err(DocError::Invalid("a trim/extend needs a boundary: a plane, a face or members".into()));
    };
    let mut notes = Vec::new();
    for name in members {
        let name = &name.0;
        if to.iter().any(|t| &t.0 == name) {
            return Err(DocError::Invalid(format!("`{name}` is a boundary member: it can't be trimmed to itself")));
        }
        let wi = record(st, name)?;
        let Some(w) = st.welds.get(wi).cloned() else { continue };
        let (Some(m), Some(stock)) = (recorded(&w), w.stock.clone()) else {
            return Err(DocError::Invalid(format!("`{name}` has no stock to make it again from")));
        };
        if m.line.closed() {
            return Err(DocError::Invalid(format!("`{name}` is a ring: it has no end to trim or extend")));
        }
        let (end, cut, new_cope) = match &boundary {
            Boundary::Plane(pl) => {
                let (end, cut) = plane_end(&m, pl, gap);
                (end, cut, None)
            }
            Boundary::Round(c) => {
                let (end, cut, cope) =
                    round_end(&m, c, gap).ok_or_else(|| DocError::Invalid(format!("`{name}` doesn't meet the round face across it")))?;
                (end, cut, Some(cope))
            }
            Boundary::Members => {
                let (k, end) = member_end(&m, &target_members).ok_or_else(|| DocError::Invalid("no boundary member".into()))?;
                let t = target_members.get(k).ok_or_else(|| DocError::Invalid("no boundary member".into()))?;
                let (p, into) = m.end(end == 0);
                // Where the member, run on past its end, meets the boundary member's centreline:
                // its direction and outline there.
                let q = t.line.nearest_to_line(p, -into);
                let (dir, outline) = t.across(q).ok_or_else(|| DocError::Invalid(format!("`{name}` doesn't meet `{}`", t.id)))?;
                let n = (into - dir * into.dot(dir)).normalized().filter(|n| n.dot(into) > MIN_JOINT_ANGLE.sin());
                let n = n.ok_or_else(|| DocError::Invalid(format!("`{name}` runs along `{}`", t.id)))?;
                if *coped {
                    // Cut through the boundary member's line, then the outside of it taken away.
                    let c = targets.get(k).and_then(|tw| cope_of(tw, t, q, n, gap)).ok_or_else(|| {
                        if t.line.path.is_empty() {
                            DocError::Invalid("a boundary member has no stock to cut against".into())
                        } else {
                            DocError::Invalid(format!("not supported yet: coping `{name}` to `{}` where it bends (cope to a straight run)", t.id))
                        }
                    })?;
                    (end, EndCut { at: q, normal: n }, Some(c))
                } else {
                    (end, EndCut { at: q + n * (extent_of(&outline, n) + gap), normal: n }, None)
                }
            }
        };
        // How far the new end plane lies past the member's end, along its direction out there.
        let (p, into) = m.end(end == 0);
        let along = cut.normal.dot(into);
        if along.abs() < MIN_JOINT_ANGLE.sin() {
            return Err(DocError::Invalid(format!("`{name}` runs along the boundary")));
        }
        let s = cut.normal.dot(p - cut.at) / along;
        let line = moved_end(&m.line, end, s).ok_or_else(|| {
            DocError::Invalid(format!("not supported yet: cutting `{name}` back into its bend (only its straight end can be cut back)"))
        })?;
        let length = line.length();
        if length < JOINT_TOL || (line.path.is_empty() && (line.b - line.a).dot(m.dir) <= 0.0) {
            return Err(DocError::Invalid(format!("the boundary cuts away all of `{name}`")));
        }
        if !length.is_finite() || length > MAX_TRIM_LENGTH {
            return Err(DocError::Invalid(format!("`{name}` would be longer than {MAX_TRIM_LENGTH} mm")));
        }
        let extend = s > JOINT_TOL;
        if (extend && *mode == crate::WeldTrimMode::Trim) || (!extend && *mode == crate::WeldTrimMode::Extend) {
            notes.push(format!("`{name}` is left as it is: the boundary would {} it", if extend { "lengthen" } else { "cut back" }));
            continue;
        }
        let mb = st.body(name).cloned().ok_or_else(|| DocError::Unknown(format!("body `{name}`")))?;
        let mut cuts = w.cuts.map(|c| EndCut { at: c.at, normal: c.normal });
        if let Some(c) = cuts.get_mut(end) {
            *c = cut;
        }
        let mut copes = w.copes.clone();
        if let Some(c) = copes.get_mut(end) {
            *c = new_cope;
        }
        // A bent member's stock frame, moved with its start.
        let mut frame = stock.frame;
        frame.origin += line.a - m.line.a;
        let reach = m.profile.iter().map(|p| p.len()).fold(0.0, f64::max);
        let remade = w.made.is(&mb.body);
        let body = if remade {
            if line.path.is_empty() {
                cut_member(&stock, &cuts, &copes)?
            } else {
                let mut body = bent_member(&line, &stock.region, &frame, reach, cuts[0], cuts[1])?;
                for c in copes.iter().flatten() {
                    body = cope(&body, c)?;
                }
                body
            }
        } else if !extend {
            let cut_back = if line.path.is_empty() {
                let plane = Plane::from_normal(cut.at, cut.normal).ok_or_else(|| DocError::Invalid("member end plane".into()))?;
                kernel::half_space(&mb.body, &plane, true)?.0.ok_or_else(|| DocError::Invalid(format!("the boundary cuts away all of `{name}`")))?
            } else {
                cut_near_end(&mb.body, p, -into, &cut, reach, 0.0)?
            };
            match copes.get(end).and_then(Option::as_ref) {
                Some(c) => cope(&cut_back, c)?,
                None => cut_back,
            }
        } else {
            return Err(DocError::Invalid(format!("not supported yet: lengthening `{name}`, which a later feature has changed")));
        };
        let moved = Member::carried(m.id, m.group, line.clone(), m.axes, m.profile.clone());
        let ((x, y), (xe, ye)) = (moved.axes, moved.end_axes);
        let mut rec = w.clone();
        rec.axis = [line.a, line.b];
        rec.length = moved.long_point_length_coped(&cuts[0], &cuts[1], &copes);
        rec.ends = [0, 1].map(|i| match (copes.get(i).and_then(Option::as_ref), cuts.get(i)) {
            (None, Some(c)) if i == 0 => crate::weld::end_of(c.normal, moved.dir, x, y),
            (None, Some(c)) => crate::weld::end_of(c.normal, moved.end_dir, xe, ye),
            _ => crate::weld::MemberEnd::Coped,
        });
        rec.leans = [crate::weld::lean_of(cuts[0].normal, moved.dir, x, y, true), crate::weld::lean_of(cuts[1].normal, moved.end_dir, xe, ye, false)];
        if let Some(bent) = &w.bent {
            let o = m.line.a - stock.frame.origin;
            let anchor = Vec2::new(stock.frame.x.dot(o), stock.frame.y.dot(o));
            rec.bent = bent_record(&moved, &cuts[0], &cuts[1], &copes, anchor, bent.allowance);
            if let Some(s) = rec.stock.as_mut() {
                s.frame = frame;
            }
        }
        rec.copes = copes;
        rec.cuts = cuts.map(|c| crate::weld::CutPlane { at: c.at, normal: c.normal });
        // A member cut as it was keeps what changed it: it is never made again from its stock.
        rec.made = crate::weld::MadeBody(remade.then(|| body.clone()));
        let made = Traced::step(body, &[&mb.traced()]);
        if let Some(slot) = st.bodies.iter_mut().find(|b| &b.name == name) {
            *slot = ModelBody::traced_new(name.clone(), made, mb.feature);
        }
        if let Some(slot) = st.welds.get_mut(wi) {
            *slot = rec;
        }
    }
    if !notes.is_empty() {
        append_warning(warning, notes.join("; "));
    }
    Ok(())
}

/// Thickest plate an End Cap or Gusset makes, and its longest leg (mm).
const MAX_PLATE: f64 = 1.0e5;

/// A plate size: an expression's length, finite and within `0 < x ≤ MAX_PLATE` (or `≥ 0` when
/// `zero` may be).
fn plate_length(vals: &BTreeMap<String, Value>, e: &Expr, what: &str, zero: bool) -> Result<f64> {
    let x = val(vals, e, Kind::Length)?;
    if x.is_finite() && x <= MAX_PLATE && (x > 1e-6 || (zero && x >= 0.0)) {
        Ok(x)
    } else if zero {
        Err(DocError::Invalid(format!("the {what} must be from 0 to {MAX_PLATE} mm")))
    } else {
        Err(DocError::Invalid(format!("the {what} must be more than 0 and at most {MAX_PLATE} mm")))
    }
}

/// The record of structural member `name` whose body is still there.
fn member_record(st: &ModelState, name: &str) -> Result<crate::weld::MemberCut> {
    st.welds
        .iter()
        .find(|w| w.body == name && st.body(name).is_some())
        .cloned()
        .ok_or_else(|| DocError::Invalid(format!("`{name}` is not a structural member")))
}

/// A flat plate: `outline` in `frame`, from `from` to `to` along its normal.
fn plate(frame: &Plane, outline: solidcraft_geom::Loop2, from: f64, to: f64) -> Result<Body> {
    let mut bodies = kernel::extrude(frame, &[Region2 { outer: outline, holes: Vec::new() }], from, to)?;
    bodies.pop().filter(|_| bodies.is_empty()).ok_or_else(|| DocError::Invalid("a plate made more than one solid".into()))
}

/// Loop `lp` under the affine map `f`: lines, Béziers and conics by their points, arcs as conics
/// of at most a quarter turn each (exact: an affine image of a circle is an ellipse).
fn map_loop(lp: &solidcraft_geom::Loop2, f: impl Fn(Vec2) -> Vec2) -> solidcraft_geom::Loop2 {
    use solidcraft_geom::Seg2;
    let mut segs = Vec::with_capacity(lp.segs.len());
    for s in &lp.segs {
        match *s {
            Seg2::Line { a, b } => segs.push(Seg2::Line { a: f(a), b: f(b) }),
            Seg2::Arc { center, radius, start, sweep } => {
                let n = (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().clamp(1.0, 8.0);
                let h = sweep / n / 2.0;
                let at = |t: f64, r: f64| center + Vec2::new(t.cos(), t.sin()) * r;
                for i in 0..n as usize {
                    let t0 = start + sweep * i as f64 / n;
                    let apex = at(t0 + h, radius / h.cos());
                    segs.push(Seg2::Conic { a: f(at(t0, radius)), apex: f(apex), b: f(at(t0 + 2.0 * h, radius)), w: h.cos() });
                }
            }
            Seg2::Cubic { p0, p1, p2, p3 } => segs.push(Seg2::Cubic { p0: f(p0), p1: f(p1), p2: f(p2), p3: f(p3) }),
            Seg2::Conic { a, apex, b, w } => segs.push(Seg2::Conic { a: f(a), apex: f(apex), b: f(b), w }),
            Seg2::Bezier { n, p } => segs.push(Seg2::Bezier { n, p: p.map(&f) }),
        }
    }
    solidcraft_geom::Loop2 { segs }
}

/// Loop `lp` with each sharp outside corner between two lines chamfered (`size` along each line)
/// or filleted (radius `size`); other corners are left as they are. `None` when the corners
/// would use up more than a line's length.
fn finish_corners(lp: &solidcraft_geom::Loop2, corner: crate::WeldCapCorner, size: f64) -> Option<solidcraft_geom::Loop2> {
    use solidcraft_geom::Seg2;
    if corner.is_square() {
        return Some(lp.clone());
    }
    let lp = lp.ccw();
    let n = lp.segs.len();
    // Joint i (seg i into seg i + 1): how far it cuts back along each line, its turn angle.
    let mut cut = vec![None; n];
    for (i, c) in cut.iter_mut().enumerate() {
        let (Some(Seg2::Line { a, b }), Some(Seg2::Line { a: b2, b: c2 })) = (lp.segs.get(i), lp.segs.get((i + 1) % n)) else { continue };
        let (Some(t), Some(u)) = ((*b - *a).normalized(), (*c2 - *b2).normalized()) else { continue };
        let turn = t.cross(u).atan2(t.dot(u));
        if turn < 1e-3 {
            continue;
        }
        let back = match corner {
            crate::WeldCapCorner::Fillet => size * (turn / 2.0).tan(),
            _ => size,
        };
        *c = Some((back, turn));
    }
    let mut segs = Vec::with_capacity(n * 2);
    for (i, s) in lp.segs.iter().enumerate() {
        let before = cut.get((i + n - 1) % n).copied().flatten();
        let here = cut.get(i).copied().flatten();
        match *s {
            Seg2::Line { a, b } if before.is_some() || here.is_some() => {
                let t = (b - a).normalized()?;
                let (from, to) = (before.map_or(0.0, |c| c.0), here.map_or(0.0, |c| c.0));
                let len = a.dist(b);
                if from + to > len + 1e-9 {
                    return None;
                }
                if len - from - to > 1e-9 {
                    segs.push(Seg2::Line { a: a + t * from, b: b - t * to });
                }
            }
            other => segs.push(other),
        }
        let (Some((back, turn)), Some(Seg2::Line { a, b }), Some(next)) = (here, lp.segs.get(i), lp.segs.get((i + 1) % n)) else { continue };
        let (t, u) = ((*b - *a).normalized()?, next.start_tangent().normalized()?);
        let (p, q) = (*b - t * back, *b + u * back);
        segs.push(match corner {
            crate::WeldCapCorner::Fillet => {
                let center = p + Vec2::new(-t.y, t.x) * size;
                let d = p - center;
                Seg2::Arc { center, radius: size, start: d.y.atan2(d.x), sweep: turn }
            }
            _ => Seg2::Line { a: p, b: q },
        });
    }
    Some(solidcraft_geom::Loop2 { segs })
}

/// The loop `lp` (either way round) set in by `inset`, counter-clockwise: the hole of a wall
/// `inset` thick inside it, so lines move square to themselves, arcs keep their centres and
/// corners stay sharp. `None` where the inset leaves no outline, or splits it in pieces.
fn inset_outline(lp: &solidcraft_geom::Loop2, inset: f64) -> Option<solidcraft_geom::Loop2> {
    let lp = lp.ccw();
    if inset <= 0.0 {
        return Some(lp);
    }
    let wall = solidcraft_sketch::thin_wall_regions(&[lp.segs], inset, 0.0).ok()?;
    let [wall] = &wall[..] else { return None };
    let [hole] = &wall.holes[..] else { return None };
    Some(hole.ccw())
}

/// Evaluation of a weldment End Cap: for each picked member end, a plate of the profile's
/// outline (set in by the inset) standing out from the end face, or of its hole's outline
/// fitted into it from the end face, its sharp outside corners chamfered or filleted. An end cut
/// at an angle takes a plate of its slanted face's shape (the outline set in square to the
/// member, the corners finished on the slanted face); a coped end takes none.
pub(super) fn end_cap(vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState) -> Result<()> {
    let FeatureKind::WeldEndCap { ends, thickness, inset, inside, corner, corner_size } = &f.kind else {
        return Err(DocError::Invalid(format!("{} is not an end cap", f.name)));
    };
    if ends.is_empty() || ends.len() > MAX_MEMBERS * 2 {
        return Err(DocError::Invalid(format!("an end cap takes 1…{} member ends", MAX_MEMBERS * 2)));
    }
    let thickness = plate_length(vals, thickness, "thickness", false)?;
    let inset = match inset {
        Some(e) => plate_length(vals, e, "inset", true)?,
        None => 0.0,
    };
    let size = match (corner.is_square(), corner_size) {
        (true, _) => 0.0,
        (false, Some(e)) => plate_length(vals, e, "corner size", false)?,
        (false, None) => return Err(DocError::Invalid("a chamfered or filleted cap needs its corner size".into())),
    };
    let mut made = Vec::with_capacity(ends.len());
    // Each plate's outline as cut, for the cut list.
    let mut cut_to = Vec::with_capacity(ends.len());
    for (i, e) in ends.iter().enumerate() {
        let name = &e.member.0;
        if ends.iter().take(i).any(|o| o == e) {
            return Err(DocError::Invalid(format!("the {} of `{name}` is picked twice", if e.end.index() == 0 { "start" } else { "end" })));
        }
        let w = member_record(st, name)?;
        if w.is_ring() {
            return Err(DocError::Invalid(format!("`{name}` is a ring: it has no open end to cap")));
        }
        let end = e.end.index();
        if w.copes.get(end).is_some_and(Option::is_some) {
            return Err(DocError::Invalid(format!("the coped end of `{name}` fits against another member: it has no open end to cap")));
        }
        let stock = w.stock.as_ref().ok_or_else(|| DocError::Invalid(format!("`{name}` has no stock to cap")))?;
        // The profile's frame at that end (a bent member's turned along it).
        let end_frame = w.frame_at(end).ok_or_else(|| DocError::Invalid(format!("`{name}` has no stock to cap")))?;
        let cut = w.cuts.get(end).copied().ok_or_else(|| DocError::Invalid("member end".into()))?;
        let d = end_frame.normal();
        let into = cut.normal.dot(d);
        if into.abs() < MIN_JOINT_ANGLE.sin() {
            return Err(DocError::Invalid(format!("the end of `{name}` is cut along the member")));
        }
        let outline = if *inside {
            stock
                .region
                .holes
                .iter()
                .max_by(|a, b| a.signed_area().abs().total_cmp(&b.signed_area().abs()))
                .ok_or_else(|| DocError::Invalid(format!("`{name}` has no hole to fit a cap into")))?
        } else {
            &stock.region.outer
        };
        let outline = inset_outline(outline, inset).ok_or_else(|| DocError::Invalid(format!("the inset leaves no cap on `{name}`")))?;
        let finish = |lp: &solidcraft_geom::Loop2| {
            finish_corners(lp, *corner, size).ok_or_else(|| DocError::Invalid(format!("the corner size is too large for the cap on `{name}`")))
        };
        let body = if into.abs() >= 1.0 - 1e-9 {
            // The profile's frame moved to the end face; the plate grows away from the member, or
            // into it when inside.
            let mut frame = end_frame;
            frame.origin += d * d.dot(cut.at - end_frame.origin);
            let grow = if *inside { into } else { -into };
            let (from, to) = if grow > 0.0 { (0.0, thickness) } else { (-thickness, 0.0) };
            let outline = finish(&outline)?;
            cut_to.push(outline.clone());
            plate(&frame, outline, from, to)?
        } else if *inside {
            // A slice of the hole, between the end face and a plane `thickness` into the member.
            let outline = finish(&outline)?;
            let reach = outline.polyline(1e-2).iter().map(|p| p.len()).fold(0.0, f64::max);
            let t = d.dot(cut.at - end_frame.origin);
            let lean = (reach * (1.0 - into * into).sqrt() + thickness) / into.abs() + 1.0;
            cut_to.push(map_loop(&outline, slant(&end_frame, d, &cut, into).1));
            let mut body = plate(&end_frame, outline, t - lean, t + lean)?;
            let near = Plane::from_normal(cut.at, cut.normal);
            let far = Plane::from_normal(cut.at + cut.normal * thickness, -cut.normal);
            for pl in [near, far] {
                let pl = pl.ok_or_else(|| DocError::Invalid("member end plane".into()))?;
                body = kernel::half_space(&body, &pl, true)?.0.ok_or_else(|| DocError::Invalid(format!("the cap on `{name}` is cut away")))?;
            }
            body
        } else {
            // The outline as the slanted end face shows it, square-edged, standing off the face.
            let (frame, flat) = slant(&end_frame, d, &cut, into);
            let frame = frame.ok_or_else(|| DocError::Invalid("member end plane".into()))?;
            let grow = -cut.normal.dot(frame.normal());
            let (from, to) = if grow > 0.0 { (0.0, thickness) } else { (-thickness, 0.0) };
            let shown = finish(&map_loop(&outline, flat))?;
            cut_to.push(shown.clone());
            plate(&frame, shown, from, to)?
        };
        made.push(body);
    }
    for (k, (body, outline)) in made.into_iter().zip(&cut_to).enumerate() {
        let name = new_name(st, f, k);
        let mut cut = crate::weld::PlateCut::new(name.clone(), f.id, crate::weld::PlateKind::EndCap, thickness, outline);
        cut.made = crate::weld::MadeBody(Some(body.clone()));
        st.plates.push(cut);
        st.bodies.push(ModelBody::traced_new(name, Traced::unnamed(body), f.id));
    }
    Ok(())
}

/// A slanted end face (cut plane `cut` across stock in `frame`, whose normal `d` meets it at
/// `into`): its plane, its axes the profile's moved onto it along the member, and the map from
/// the profile's points to the face's.
fn slant(frame: &Plane, d: Vec3, cut: &crate::weld::CutPlane, into: f64) -> (Option<Plane>, impl Fn(Vec2) -> Vec2) {
    let (at, n, f) = (cut.at, cut.normal, *frame);
    let along = move |p: Vec3| p + d * (n.dot(at - p) / into);
    let o = along(f.origin);
    let face = Plane::new(o, along(f.origin + f.x) - o, along(f.origin + f.y) - o);
    let (x, y) = face.map_or((Vec3::X, Vec3::Y), |p| (p.x, p.y));
    (face, move |q: Vec2| {
        let p = along(f.to_world(q)) - o;
        Vec2::new(x.dot(p), y.dot(p))
    })
}

/// Where the lines of straight members `a` and `b` (named `an` and `bn`) cross, for a gusset.
fn gusset_corner(a: &Member, b: &Member, an: &str, bn: &str) -> Result<Vec3> {
    // Where the two lines come closest.
    let w0 = a.line.a - b.line.a;
    let (ab, aw0, bw0) = (a.dir.dot(b.dir), a.dir.dot(w0), b.dir.dot(w0));
    let den = 1.0 - ab * ab;
    if den < MIN_JOINT_ANGLE.sin().powi(2) {
        return Err(DocError::Invalid(format!("`{an}` and `{bn}` run side by side: there is no corner for a gusset")));
    }
    let (sa, sb) = ((ab * bw0 - aw0) / den, (bw0 - ab * aw0) / den);
    let (p, q) = (a.line.a + a.dir * sa, b.line.a + b.dir * sb);
    let reach = |m: &Member, s: f64| {
        let r = m.outline.iter().map(|o| o.len()).fold(0.0, f64::max) + JOINT_TOL;
        s >= -r && s <= m.line.a.dist(m.line.b) + r
    };
    if p.dist(q) > 1e-2 || !reach(a, sa) || !reach(b, sb) {
        return Err(DocError::Invalid(format!("`{an}` and `{bn}` don't meet: a gusset needs their lines to cross")));
    }
    Ok((p + q) * 0.5)
}

/// The straight pieces of member `m` a gusset can stand on, each as a straight member with the
/// profile's outline there: a straight member itself, a bent one's straight runs (none on a
/// member formed to a free-form shape).
fn straight_runs<'a>(m: &Member<'a>) -> Vec<Member<'a>> {
    if m.line.path.is_empty() {
        return vec![m.clone()];
    }
    if m.line.formed {
        return Vec::new();
    }
    m.line
        .path
        .iter()
        .filter_map(|s| match *s {
            kernel::PathSeg::Line { a, b } => {
                let (d, axes) = carry(&m.line, m.axes, Some(a.lerp(b, 0.5)))?;
                Some(Member::straight(m.id, m.group, Line::straight(a, b, m.line.up), d, axes, m.offsets(axes)))
            }
            kernel::PathSeg::Arc { .. } => None,
        })
        .collect()
}

/// Evaluation of a weldment Gusset: a plate in the corner between two members whose lines
/// meet, on the plane of their lines (centred, or to one side, and offset), its two legs lying
/// along the members' faces from the corner where those faces meet: toward each member's far
/// end, or reversed. The corner is clipped for the weld, and the sides may stand square off each
/// face before the slanted edge. The plane's normal (its `one` side) is the first leg's direction
/// crossed with the second's.
pub(super) fn gusset(vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState) -> Result<()> {
    let FeatureKind::WeldGusset { members, size, size2, thickness, edge, clip, offset, side, reverse_first, reverse_second } = &f.kind else {
        return Err(DocError::Invalid(format!("{} is not a gusset", f.name)));
    };
    let [an, bn] = members;
    if an == bn {
        return Err(DocError::Invalid("a gusset needs two different members".into()));
    }
    let (aw, bw) = (member_record(st, &an.0)?, member_record(st, &bn.0)?);
    let no_stock = |n: &str| DocError::Invalid(format!("`{n}` has no profile to set a gusset against"));
    let a = aw.stock.as_ref().and_then(|_| recorded(&aw)).ok_or_else(|| no_stock(&an.0))?;
    let b = bw.stock.as_ref().and_then(|_| recorded(&bw)).ok_or_else(|| no_stock(&bn.0))?;
    let d1 = plate_length(vals, size, "size", false)?;
    let d2 = match size2 {
        Some(e) => plate_length(vals, e, "second size", false)?,
        None => d1,
    };
    let thickness = plate_length(vals, thickness, "thickness", false)?;
    let edge = match edge {
        Some(e) => plate_length(vals, e, "edge height", true)?,
        None => 0.0,
    };
    let clip = match clip {
        Some(e) => plate_length(vals, e, "corner clip", true)?,
        None => 0.0,
    };
    let offset = match offset {
        Some(e) => val(vals, e, Kind::Length)?,
        None => 0.0,
    };
    if !(offset.is_finite() && offset.abs() <= MAX_PLATE) {
        return Err(DocError::Invalid(format!("the offset must be within ±{MAX_PLATE} mm")));
    }
    // A bent member's straight run that meets the other member's.
    let (a, b, at) = if a.line.path.is_empty() && b.line.path.is_empty() {
        let at = gusset_corner(&a, &b, &an.0, &bn.0)?;
        (a, b, at)
    } else {
        let mut best: Option<(f64, Member, Member, Vec3)> = None;
        for ra in straight_runs(&a) {
            for rb in straight_runs(&b) {
                if let Ok(at) = gusset_corner(&ra, &rb, &an.0, &bn.0) {
                    // The runs that meet nearest their own ends (where a bent member's runs
                    // touch each other, the one that holds the corner).
                    let d = [ra.line.a, ra.line.b, rb.line.a, rb.line.b].iter().map(|p| p.dist(at)).fold(f64::INFINITY, f64::min);
                    if best.as_ref().is_none_or(|(bd, ..)| d < *bd) {
                        best = Some((d, ra.clone(), rb.clone(), at));
                    }
                }
            }
        }
        let (_, ra, rb, at) = best.ok_or_else(|| {
            DocError::Invalid(format!(
                "`{}` and `{}` don't meet on straight runs: a gusset on a bent member stands on a straight run of it",
                an.0, bn.0
            ))
        })?;
        (ra, rb, at)
    };
    // Away from the joint along each member: toward its far end, or reversed toward its near one.
    let away = |m: &Member, reverse: bool| {
        let far = if at.dist(m.line.a) < at.dist(m.line.b) { m.dir } else { -m.dir };
        if reverse { -far } else { far }
    };
    let (u, v) = (away(&a, *reverse_first), away(&b, *reverse_second));
    // In the plate's plane, square to each member toward the other.
    let bad = || DocError::Invalid("the gusset's corner".into());
    let wa = (v - u * v.dot(u)).normalized().ok_or_else(bad)?;
    let wb = (u - v * u.dot(v)).normalized().ok_or_else(bad)?;
    // The corner where the two members' faces toward each other meet.
    let (cu, cv) = (b.extent(wb) / u.dot(wb), a.extent(wa) / v.dot(wa));
    let corner = at + u * cu + v * cv;
    // A reversed leg needs its member to run on past the corner that way.
    let runs =
        |m: &Member, dir: Vec3, c: f64| [m.line.a, m.line.b].iter().map(|p| (*p - at).dot(dir)).fold(f64::NEG_INFINITY, f64::max) > c + JOINT_TOL;
    for (m, dir, c, n) in [(&a, u, cu, an), (&b, v, cv, bn)] {
        if !runs(m, dir, c) {
            return Err(DocError::Invalid(format!("`{}` doesn't run on past the joint that way: reverse its leg back", n.0)));
        }
    }
    let plane = Plane::new(corner, u, wa).ok_or_else(bad)?;
    let flat = |p: Vec3| Vec2::new(plane.x.dot(p - corner), plane.y.dot(p - corner));
    let mut pts = Vec::with_capacity(6);
    if clip > 0.0 {
        pts.extend([corner + v * clip, corner + u * clip]);
    } else {
        pts.push(corner);
    }
    pts.push(corner + u * d1);
    if edge > 0.0 {
        pts.extend([corner + u * d1 + wa * edge, corner + v * d2 + wb * edge]);
    }
    pts.push(corner + v * d2);
    let pts: Vec<Vec2> = pts.into_iter().map(flat).collect();
    // A convex outline, counter-clockwise in the plate's frame (u then toward v).
    let n = pts.len();
    let convex = (0..n).all(|i| match (pts.get(i), pts.get((i + 1) % n), pts.get((i + 2) % n)) {
        (Some(p0), Some(p1), Some(p2)) => (*p1 - *p0).cross(*p2 - *p1) > 1e-9,
        _ => false,
    });
    if !(d1 > clip && d2 > clip && convex) {
        return Err(DocError::Invalid("the gusset's corner clip or edge height is too large for its size".into()));
    }
    let outline = solidcraft_geom::Loop2::polygon(&pts);
    let region = Region2 { outer: outline.clone(), holes: Vec::new() };
    let (from, to) = match side {
        crate::WeldPlateSide::Center => (offset - thickness / 2.0, offset + thickness / 2.0),
        crate::WeldPlateSide::One => (offset, offset + thickness),
        crate::WeldPlateSide::Other => (offset - thickness, offset),
    };
    let mut bodies = kernel::extrude(&plane, &[region], from, to)?;
    let body = bodies.pop().filter(|_| bodies.is_empty()).ok_or_else(|| DocError::Invalid("a gusset made more than one solid".into()))?;
    let name = new_name(st, f, 0);
    let mut cut = crate::weld::PlateCut::new(name.clone(), f.id, crate::weld::PlateKind::Gusset, thickness, &outline);
    cut.made = crate::weld::MadeBody(Some(body.clone()));
    st.plates.push(cut);
    st.bodies.push(ModelBody::traced_new(name, Traced::unnamed(body), f.id));
    Ok(())
}
