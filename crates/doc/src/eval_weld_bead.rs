//! Evaluation of weldment Weld Beads: the seams where two bodies meet, found from their meshes.
//! A seam is a stretch of either body's edge where the other body lies just across the edge's
//! face and the corner beside it is open: a fillet or groove weld can go there. Edges round a
//! hole in the face (a tube's bore standing on a plate) are not seams, nor are seams no line of
//! sight from outside the two bodies reaches (inside a tube, behind a cap fitted into it).

use super::*;
use crate::Expr;
use solidcraft_geom::Mesh;

/// Largest bead size, root gap and intermittent length or pitch (mm).
const MAX_BEAD: f64 = 1.0e4;

/// Most edge pieces of the two bodies one bead looks at, and most seam pieces it keeps.
const MAX_PIECES: usize = 100_000;
const MAX_SEAM_PIECES: usize = 20_000;

/// Most other bodies near the two whose corners a bead looks at.
const MAX_OTHERS: usize = 64;

/// Points an edge piece is looked at along before its seam's ends are found more closely.
const PIECE_SAMPLES: usize = 16;

/// Depths a seam test looks across a root gap at, touching to the full gap, and the most apart
/// two of them may be (mm): a wall thinner than the gap (a tube's) is found wherever it stands.
const GAP_DEPTHS: usize = 32;
const GAP_DEPTH_STEP: f64 = 0.5;

/// Halvings that find where a seam starts or stops along an edge piece.
const BISECTIONS: usize = 24;

/// Most points a bead's `at` lists.
const MAX_AT: usize = 256;

/// Longest note (characters).
const MAX_NOTE: usize = 256;

/// Directions a seam is looked at from to tell whether it can be reached, and the share of the
/// open ones (not into either body) that must see out past both bodies.
const SIGHT_RAYS: usize = 64;
const MIN_SIGHT: f64 = 0.1;

/// A length of a bead: finite, within `0 < x ≤ MAX_BEAD` (or `≥ 0` when `zero` may be).
fn bead_length(vals: &BTreeMap<String, Value>, e: &Expr, what: &str, zero: bool) -> Result<f64> {
    let x = val(vals, e, Kind::Length)?;
    if x.is_finite() && x <= MAX_BEAD && (x > 1e-6 || (zero && x >= 0.0)) {
        Ok(x)
    } else if zero {
        Err(DocError::Invalid(format!("the {what} must be from 0 to {MAX_BEAD} mm")))
    } else {
        Err(DocError::Invalid(format!("the {what} must be more than 0 and at most {MAX_BEAD} mm")))
    }
}

/// Evaluation of a Weld Bead: the seams between its two bodies (those nearest `at`, if given),
/// recorded for the weld table and the view. It adds no material and changes no body.
pub(super) fn bead(vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState) -> Result<()> {
    let FeatureKind::WeldBead { bodies, size, kind, at, gap, length, pitch, field, note } = &f.kind else {
        return Err(DocError::Invalid(format!("{} is not a weld bead", f.name)));
    };
    let [an, bn] = bodies;
    if an == bn {
        return Err(DocError::Invalid("a weld bead needs two different bodies".into()));
    }
    let body = |n: &str| st.body(n).map(|b| b.body.clone()).ok_or_else(|| DocError::Invalid(format!("`{n}` is not a body")));
    let (a, b) = (body(&an.0)?, body(&bn.0)?);
    // The other bodies near the two: a corner one of them fills is not open to weld.
    let around = a.outline_bounds().union(&b.outline_bounds()).inflated(1.0);
    let others: Vec<Arc<Mesh>> = st
        .bodies
        .iter()
        .filter(|x| x.name != an.0 && x.name != bn.0 && x.body.outline_bounds().meets(&around))
        .take(MAX_OTHERS)
        .map(ModelBody::mesh)
        .collect();
    let size = bead_length(vals, size, "bead size", false)?;
    let gap = match gap {
        Some(e) => bead_length(vals, e, "gap", true)?,
        None => 0.0,
    };
    let intermittent = match (length, pitch) {
        (None, None) => None,
        (Some(l), Some(p)) => {
            let (l, p) = (bead_length(vals, l, "weld length", false)?, bead_length(vals, p, "pitch", false)?);
            if p < l - 1e-9 {
                return Err(DocError::Invalid("the pitch of intermittent welds must be at least their length".into()));
            }
            Some([l, p])
        }
        _ => return Err(DocError::Invalid("intermittent welds need both a length and a pitch".into())),
    };
    if note.as_ref().is_some_and(|n| n.chars().count() > MAX_NOTE) {
        return Err(DocError::Invalid(format!("a weld note is at most {MAX_NOTE} characters")));
    }
    if at.len() > MAX_AT || at.iter().any(|p| !p.0.is_finite()) {
        return Err(DocError::Invalid(format!("a weld bead takes at most {MAX_AT} finite points")));
    }
    let mut seams = seams(&a, &b, &others, gap)?;
    if seams.is_empty() {
        return Err(DocError::Invalid(format!(
            "`{}` and `{}` don't meet: there is no seam to weld (bodies standing apart need the gap between them)",
            an.0, bn.0
        )));
    }
    if !at.is_empty() {
        // The seam nearest each point; a point far from every seam is a mistake, not a choice.
        let reach = 10.0 + 2.0 * size + gap;
        let mut keep = vec![false; seams.len()];
        for p in at {
            let near = seams
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (i, s.windows(2).map(|w| if let [x, y] = w { p.0.dist_to_segment(*x, *y) } else { f64::INFINITY }).fold(f64::INFINITY, f64::min))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1));
            match near {
                Some((i, d)) if d <= reach => {
                    if let Some(k) = keep.get_mut(i) {
                        *k = true;
                    }
                }
                _ => {
                    return Err(DocError::Invalid(format!(
                        "no seam between `{}` and `{}` near ({:.1}, {:.1}, {:.1})",
                        an.0, bn.0, p.0.x, p.0.y, p.0.z
                    )));
                }
            }
        }
        let mut k = keep.into_iter();
        seams.retain(|_| k.next().unwrap_or(false));
    }
    if seams.len() > crate::weld::MAX_SEAMS {
        return Err(DocError::Invalid(format!("a weld bead welds at most {} seams; pick some with points", crate::weld::MAX_SEAMS)));
    }
    st.beads.push(crate::weld::Bead {
        feature: f.id,
        name: f.name.clone(),
        bodies: [an.0.clone(), bn.0.clone()],
        kind: *kind,
        size,
        seams,
        intermittent,
        field: *field,
        note: note.clone().filter(|n| !n.trim().is_empty()),
    });
    Ok(())
}

/// The seams where bodies `a` and `b` meet (standing up to `gap` apart), each a polyline: a
/// closed one ends where it starts. A corner one of `others` fills is no seam.
fn seams(a: &Body, b: &Body, others: &[Arc<Mesh>], gap: f64) -> Result<Vec<Vec<Vec3>>> {
    let size = a.size().max(b.size());
    if !(size.is_finite() && size > 0.0) {
        return Ok(Vec::new());
    }
    let tol = (size * 2e-5).max(1e-4);
    let (ma, mb) = (a.tessellate(tol)?, b.tessellate(tol)?);
    // How far off a face a point is looked at: past the facets' sag from the true surface.
    let step = tol * 4.0;
    let mut pieces: Vec<(Vec3, Vec3)> = Vec::new();
    let mut work = 0usize;
    for (x, y) in [(&ma, &mb), (&mb, &ma)] {
        let found = edge_seams(x, y, others, step, gap, &mut work)?;
        // The other body's pieces along the same seam (a mitre's two faces share their outline).
        let first = pieces.len();
        for (p, q) in found {
            let mid = (p + q) * 0.5;
            if !pieces.iter().take(first).any(|(s, t)| mid.dist_to_segment(*s, *t) <= step + gap) {
                pieces.push((p, q));
            }
            if pieces.len() > MAX_SEAM_PIECES {
                return Err(DocError::Invalid("the bodies meet along too many edges to weld as one bead".into()));
            }
        }
    }
    Ok(chain(pieces, tol))
}

/// The pieces of `x`'s edges that are seams with `y`.
fn edge_seams(x: &Mesh, y: &Mesh, others: &[Arc<Mesh>], step: f64, gap: f64, work: &mut usize) -> Result<Vec<(Vec3, Vec3)>> {
    let near: solidcraft_geom::Aabb3 = y.positions.iter().copied().collect();
    let near = near.inflated(2.0 * (step + gap));
    let mut tris_of: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, f) in x.tri_face.iter().enumerate() {
        tris_of.entry(*f).or_default().push(i);
    }
    let mut outer: BTreeMap<u32, std::collections::BTreeSet<usize>> = BTreeMap::new();
    let mut out = Vec::new();
    for (ei, e) in x.edges.iter().enumerate() {
        // A seam between two pieces of one smooth surface is no edge to weld along.
        if x.seams.get(ei).copied().unwrap_or(false) || !e.iter().any(|p| near.contains(*p)) {
            continue;
        }
        let faces: Vec<u32> = x.edge_faces.get(ei).into_iter().flatten().copied().filter(|fi| outer_edge(x, *fi, ei, &mut outer)).collect();
        let mut found = Vec::new();
        for w in e.windows(2) {
            let [p, q] = w else { continue };
            *work += 1;
            if *work > MAX_PIECES {
                return Err(DocError::Invalid("the bodies have too many edges to look for seams along".into()));
            }
            let Some(t) = (*q - *p).normalized() else { continue };
            // Each face's frame along the piece (from its middle: a curved face's pieces are short).
            let mid = (*p + *q) * 0.5;
            let frames: Vec<(Vec3, Vec3)> =
                faces.iter().filter_map(|fi| face_frame(x, tris_of.get(fi).map(Vec::as_slice).unwrap_or_default(), mid, t)).collect();
            if frames.is_empty() {
                continue;
            }
            // The other body just across a face (anywhere from touching to `gap` off it), a little
            // in from the edge, and the corner beside the edge open (in neither body nor any
            // other). Both points lean mostly along the face: at a mitre's corners, as far off it
            // as along would land on a wall's plane.
            let depths = if gap > 0.0 { ((gap / GAP_DEPTH_STEP).ceil() as usize).clamp(1, GAP_DEPTHS) } else { 0 };
            let seam_at = |m: Vec3| {
                frames.iter().any(|(n, inward)| {
                    let across = m + *inward * (2.0 * (step + gap)) + *n * (step * 0.25);
                    let beside = m - *inward * step - *n * (step * 0.25);
                    !inside(x, beside)
                        && !inside(y, beside)
                        && !others.iter().any(|o| inside(o, beside))
                        && (0..=depths).any(|k| inside(y, across + *n * (gap * k as f64 / depths.max(1) as f64)))
                })
            };
            found.extend(seam_runs(*p, *q, seam_at));
        }
        if !found.is_empty() && in_sight(x, y, others, &found, step) {
            out.extend(found);
        }
    }
    Ok(out)
}

/// The stretches of the piece from `p` to `q` where `seam_at` holds: looked at in
/// [`PIECE_SAMPLES`] places, each change between two of them found by halving.
fn seam_runs(p: Vec3, q: Vec3, seam_at: impl Fn(Vec3) -> bool) -> Vec<(Vec3, Vec3)> {
    let at = |s: f64| p.lerp(q, s);
    let n = PIECE_SAMPLES;
    let marks: Vec<(f64, bool)> = (0..n).map(|i| (i as f64 + 0.5) / n as f64).map(|s| (s, seam_at(at(s)))).collect();
    // Where the answer changes between two samples, closer by halving.
    let change = |mut lo: f64, mut hi: f64, lo_seam: bool| {
        for _ in 0..BISECTIONS {
            let mid = 0.5 * (lo + hi);
            if seam_at(at(mid)) == lo_seam {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    };
    let mut runs = Vec::new();
    let mut start: Option<f64> = marks.first().filter(|m| m.1).map(|_| 0.0);
    for w in marks.windows(2) {
        let [(s0, a), (s1, b)] = w else { continue };
        if a != b {
            let s = change(*s0, *s1, *a);
            if *b {
                start = Some(s);
            } else if let Some(from) = start.take() {
                runs.push((from, s));
            }
        }
    }
    if let Some(from) = start {
        runs.push((from, 1.0));
    }
    runs.into_iter().filter(|(a, b)| b - a > 1e-9).map(|(a, b)| (at(a), at(b))).collect()
}

/// The outward normal of face `tris` (triangles of `x`) at `m` on its edge running along `t`,
/// and the direction from the edge into the face, square to it.
fn face_frame(x: &Mesh, tris: &[usize], m: Vec3, t: Vec3) -> Option<(Vec3, Vec3)> {
    let mut best: Option<(f64, [Vec3; 3])> = None;
    for i in tris {
        let Some(c) = x.triangles.get(*i).and_then(|tri| x.tri(tri)) else { continue };
        let [a, b, cc] = c;
        let d = m.dist_to_segment(a, b).min(m.dist_to_segment(b, cc)).min(m.dist_to_segment(cc, a));
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    let (_, [a, b, c]) = best?;
    let n = (b - a).cross(c - a).normalized()?;
    let w = n.cross(t).normalized()?;
    let centre = (a + b + c) * (1.0 / 3.0);
    Some((n, if (centre - m).dot(w) >= 0.0 { w } else { -w }))
}

/// Whether edge `ei` of `x` lies on face `face`'s outer boundary (not round a hole in it): the
/// face's edges joined end to end into loops, the longest loop being the outer one.
fn outer_edge(x: &Mesh, face: u32, ei: usize, cache: &mut BTreeMap<u32, std::collections::BTreeSet<usize>>) -> bool {
    if let Some(set) = cache.get(&face) {
        return set.contains(&ei);
    }
    let edges: Vec<usize> = x.edge_faces.iter().enumerate().filter(|(_, f)| f.contains(&face)).map(|(i, _)| i).take(4096).collect();
    let ends = |i: usize| x.edges.get(i).and_then(|e| Some((*e.first()?, *e.last()?)));
    let len = |i: usize| x.edges.get(i).map_or(0.0, |e| e.windows(2).map(|w| if let [a, b] = w { a.dist(*b) } else { 0.0 }).sum());
    // Loops: edges sharing an end, by a union of their groups.
    let mut group: Vec<usize> = (0..edges.len()).collect();
    fn root(g: &mut [usize], mut i: usize) -> usize {
        while let Some(&p) = g.get(i) {
            if p == i {
                break;
            }
            i = p;
        }
        i
    }
    let span = edges.iter().filter_map(|i| ends(*i)).map(|(a, b)| a.dist(b)).fold(0.0, f64::max);
    let eps = (span * 1e-6).max(1e-6);
    for (i, ei) in edges.iter().enumerate() {
        for (j, ej) in edges.iter().enumerate().skip(i + 1) {
            let (Some((a0, a1)), Some((b0, b1))) = (ends(*ei), ends(*ej)) else { continue };
            if [a0.dist(b0), a0.dist(b1), a1.dist(b0), a1.dist(b1)].iter().any(|d| *d <= eps) {
                let (ri, rj) = (root(&mut group, i), root(&mut group, j));
                if let Some(g) = group.get_mut(ri) {
                    *g = rj;
                }
            }
        }
    }
    let mut loops: BTreeMap<usize, (f64, Vec<usize>)> = BTreeMap::new();
    for (i, e) in edges.iter().enumerate() {
        let r = root(&mut group, i);
        let l = loops.entry(r).or_default();
        l.0 += len(*e);
        l.1.push(*e);
    }
    let set: std::collections::BTreeSet<usize> =
        loops.into_values().max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, e)| e.into_iter().collect()).unwrap_or_default();
    let on = set.contains(&ei);
    cache.insert(face, set);
    on
}

/// Whether `p` is inside the closed mesh `m`, by its winding number there: the solid angle its
/// triangles fill seen from `p` (van Oosterom and Strackee), over a full sphere. Unlike counting
/// a ray's crossings it can't miscount where the ray runs through a triangle's edge, which points
/// a hair off a face (as the seam tests look at) are prone to.
fn inside(m: &Mesh, p: Vec3) -> bool {
    let mut angle = 0.0;
    for t in &m.triangles {
        let Some([a, b, c]) = m.tri(t) else { continue };
        let (a, b, c) = (a - p, b - p, c - p);
        let (la, lb, lc) = (a.len(), b.len(), c.len());
        let num = a.dot(b.cross(c));
        let den = la * lb * lc + a.dot(b) * lc + b.dot(c) * la + c.dot(a) * lb;
        angle += 2.0 * num.atan2(den);
    }
    angle > 2.0 * std::f64::consts::PI
}

/// Whether the seam along `pieces` (of `x`'s edge) can be seen from outside the bodies: from its
/// middle, enough of the directions open there see out past `x`, `y` and `others`.
fn in_sight(x: &Mesh, y: &Mesh, others: &[Arc<Mesh>], pieces: &[(Vec3, Vec3)], step: f64) -> bool {
    let Some((p, q)) = pieces.get(pieces.len() / 2) else { return false };
    let m = (*p + *q) * 0.5;
    let (mut open, mut seen) = (0usize, 0usize);
    // Directions spread evenly over the sphere (a Fibonacci lattice).
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    for k in 0..SIGHT_RAYS {
        let z = 1.0 - (k as f64 + 0.5) * 2.0 / SIGHT_RAYS as f64;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let a = golden * k as f64;
        let d = Vec3::new(r * a.cos(), r * a.sin(), z);
        let from = m + d * (2.0 * step);
        if inside(x, from) || inside(y, from) || others.iter().any(|o| inside(o, from)) {
            continue;
        }
        open += 1;
        if x.ray_crossings(from, d) == 0 && y.ray_crossings(from, d) == 0 && others.iter().all(|o| o.ray_crossings(from, d) == 0) {
            seen += 1;
        }
    }
    seen >= 2 && seen as f64 >= MIN_SIGHT * open as f64
}

/// Seam pieces joined end to end into polylines (ends within `tol`); a closed one ends where it
/// starts.
fn chain(pieces: Vec<(Vec3, Vec3)>, tol: f64) -> Vec<Vec<Vec3>> {
    let eps = tol * 2.0;
    let cell = |p: Vec3| [(p.x / eps).round() as i64, (p.y / eps).round() as i64, (p.z / eps).round() as i64];
    let mut at: std::collections::HashMap<[i64; 3], Vec<usize>> = std::collections::HashMap::new();
    for (i, (p, q)) in pieces.iter().enumerate() {
        at.entry(cell(*p)).or_default().push(i);
        at.entry(cell(*q)).or_default().push(i);
    }
    let mut used = vec![false; pieces.len()];
    // An unused piece with an end at `p`, and its other end.
    let next = |p: Vec3, used: &[bool]| -> Option<(usize, Vec3)> {
        let [cx, cy, cz] = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for i in at.get(&[cx + dx, cy + dy, cz + dz]).into_iter().flatten() {
                        if used.get(*i).copied().unwrap_or(true) {
                            continue;
                        }
                        let Some((a, b)) = pieces.get(*i) else { continue };
                        if a.dist(p) <= eps {
                            return Some((*i, *b));
                        }
                        if b.dist(p) <= eps {
                            return Some((*i, *a));
                        }
                    }
                }
            }
        }
        None
    };
    let mut out = Vec::new();
    for start in 0..pieces.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Some((a, b)) = pieces.get(start).copied() else { continue };
        if let Some(u) = used.get_mut(start) {
            *u = true;
        }
        let mut line = std::collections::VecDeque::from([a, b]);
        while let Some((i, p)) = line.back().and_then(|e| next(*e, &used)) {
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            line.push_back(p);
        }
        while let Some((i, p)) = line.front().and_then(|e| next(*e, &used)) {
            if let Some(u) = used.get_mut(i) {
                *u = true;
            }
            line.push_front(p);
        }
        let mut line: Vec<Vec3> = line.into_iter().collect();
        // Points along one straight stretch say nothing more than its ends.
        line = straightened(line, tol);
        if line.len() >= 2 {
            out.push(line);
        }
    }
    out
}

/// `line` without the points lying on the straight between their neighbours (within `tol`).
fn straightened(line: Vec<Vec3>, tol: f64) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::with_capacity(line.len());
    for p in line {
        while out.len() >= 2 {
            let (Some(a), Some(b)) = (out.get(out.len() - 2), out.last()) else { break };
            if b.dist_to_segment(*a, p) <= tol * 0.1 {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}
