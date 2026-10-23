//! Weldments: Structural Member (a closed profile along straight skeleton lines, one body each,
//! mitred or butted where members meet) and the cut list of the members.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_doc::expr::Kind;
use solidcraft_doc::metadata::MAX_KEYS;
use solidcraft_doc::weld::{Bead, Likeness, MemberCut, MemberEnd, PlateCut, PlateKind, bend_steps, likeness, member_lines, same_bends};
use solidcraft_doc::{
    BodyName, Expr, FaceAt, FeatureKind, Position, SketchId, WeldBeadKind, WeldCapCorner, WeldCorner, WeldEndRef, WeldEndSide, WeldGroup, WeldJoint,
    WeldLocate, WeldMirror, WeldPlateSide, WeldTrimMode,
};

use super::CommandSpec;
use super::bom::property;
use super::features::{add_feature, check_expr, plane_param, profiles, sketch_id};
use crate::params::{bad, bool_, expr, num, str_, string_list};
use crate::{EngineError, Result, Session};

fn trim_extend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.trim_extend";
    let st = s.model.state();
    let member_names = |key: &str| -> Result<Vec<BodyName>> {
        let names = string_list(p, key);
        if names.len() > solidcraft_doc::MAX_WELD_MEMBERS {
            return Err(bad(cmd, format!("`{key}` takes at most {} members", solidcraft_doc::MAX_WELD_MEMBERS)));
        }
        for n in &names {
            if !st.welds.iter().any(|w| &w.body == n) || st.body(n).is_none() {
                return Err(bad(cmd, format!("`{n}` is not a structural member")));
            }
        }
        Ok(names.into_iter().map(BodyName).collect())
    };
    let members = member_names("members")?;
    if members.is_empty() {
        return Err(bad(cmd, "`members` must list the structural members to trim or extend"));
    }
    let to = member_names("to")?;
    let face = match p.get("face").filter(|v| !v.is_null()) {
        Some(t) => {
            let body = t.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`face.body` must name a body"))?.to_string();
            let point = t.get("point").and_then(crate::params::vec3).ok_or_else(|| bad(cmd, "`face.point` must be [x, y, z] on the face"))?;
            let b = st.body(&body).ok_or_else(|| bad(cmd, format!("no body `{body}`")))?;
            let i = solidcraft_doc::appearance::face_index_at(b, point).ok_or_else(|| bad(cmd, "`face.point` is not on a face of the body"))?;
            if b.body.face_plane(i).or_else(|| b.body.planar_face(i)).is_none() && !flag(p, "cope", cmd)? {
                return Err(bad(cmd, "not supported yet: cutting flat to a curved face (set `cope` to cope to a round one)"));
            }
            Some(FaceAt { body: BodyName(body), point: Position(point) })
        }
        None => None,
    };
    let plane = match p.get("plane").filter(|v| !v.is_null()) {
        Some(v) => Some(plane_param(s, Some(v), cmd)?),
        None => None,
    };
    if usize::from(!to.is_empty()) + usize::from(face.is_some()) + usize::from(plane.is_some()) != 1 {
        return Err(bad(cmd, "give one boundary: `plane`, `face` or `to`"));
    }
    let mode = match str_(p, "mode").map(str::to_ascii_lowercase).as_deref() {
        None | Some("both") => WeldTrimMode::Both,
        Some("trim") => WeldTrimMode::Trim,
        Some("extend") => WeldTrimMode::Extend,
        _ => return Err(bad(cmd, "`mode` must be both, trim or extend")),
    };
    let gap = expr(p, "gap");
    if let Some(g) = &gap {
        check_expr(s, g, Kind::Length, cmd, "gap")?;
    }
    let cope = flag(p, "cope", cmd)?;
    if cope && plane.is_some() {
        return Err(bad(cmd, "`cope` needs members (`to`) or a round face (`face`) as the boundary"));
    }
    add_feature(s, p, FeatureKind::WeldTrim { members, plane, face, to, mode, gap, cope })
}

/// An optional true/false parameter (default false).
fn flag(p: &Value, key: &str, cmd: &str) -> Result<bool> {
    flag_or(p, key, false, cmd)
}

/// An optional true/false parameter, `default` where it isn't given.
fn flag_or(p: &Value, key: &str, default: bool, cmd: &str) -> Result<bool> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(_) => bool_(p, key).ok_or_else(|| bad(cmd, format!("`{key}` must be true or false"))),
    }
}

/// Lengths a stock is bought in, `p[key]`: a list of lengths (bare numbers in the design's unit,
/// or with their own), in mm, longest first, each once; null or empty is none.
pub(super) fn stock_lengths(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<Vec<f64>> {
    let given = match p.get(key) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(v)) if v.len() <= solidcraft_doc::weld::MAX_STOCK_LENGTHS => v,
        Some(_) => {
            return Err(bad(cmd, format!("`{key}` must list at most {} lengths", solidcraft_doc::weld::MAX_STOCK_LENGTHS)));
        }
    };
    let mut out = Vec::new();
    for v in given {
        let e = expr(&json!({"l": v}), "l").ok_or_else(|| bad(cmd, format!("`{key}` must list lengths")))?;
        let mm = s.doc.eval(&s.doc.with_design_unit(&e, Kind::Length), Kind::Length).map_err(|err| bad(cmd, format!("{key}: {err}")))?;
        out.push(mm);
    }
    solidcraft_doc::weld::check_lengths(&out).map_err(|e| bad(cmd, e))?;
    out.sort_by(|a, b| b.total_cmp(a));
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    Ok(out)
}

/// A length parameter, checked; `None` where it isn't given and needn't be.
fn length(s: &Session, p: &Value, key: &str, required: bool, cmd: &str) -> Result<Option<Expr>> {
    match expr(p, key) {
        Some(e) => {
            check_expr(s, &e, Kind::Length, cmd, key)?;
            Ok(Some(e))
        }
        None if required => Err(bad(cmd, format!("`{key}` is required"))),
        None => Ok(None),
    }
}

/// `p` with `body_name` set to `base` (numbered past the bodies there) when it names none.
fn default_name(s: &Session, p: &Value, base: &str) -> Value {
    let mut p = p.clone();
    if str_(&p, "body_name").is_none() && string_list(&p, "body_names").is_empty() {
        p["body_name"] = json!(free_name(s, base));
    }
    p
}

fn end_cap(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.end_cap";
    let st = s.model.state();
    let mut ends = Vec::new();
    let record = |name: &str| {
        st.welds.iter().find(|w| w.body == name && st.body(name).is_some()).ok_or_else(|| bad(cmd, format!("`{name}` is not a structural member")))
    };
    if let Some(list) = p.get("ends").filter(|v| !v.is_null()) {
        let list = list.as_array().ok_or_else(|| bad(cmd, "`ends` must be a list of {member, end}"))?;
        for e in list {
            let member = str_(e, "member").ok_or_else(|| bad(cmd, "each end needs `member`: a structural member body"))?;
            record(member)?;
            let end = match str_(e, "end") {
                Some("start") => WeldEndSide::Start,
                Some("end") => WeldEndSide::End,
                _ => return Err(bad(cmd, "each end's `end` must be start or end")),
            };
            ends.push(WeldEndRef { member: BodyName(member.to_string()), end });
        }
    }
    if let Some(list) = p.get("faces").filter(|v| !v.is_null()) {
        let list = list.as_array().ok_or_else(|| bad(cmd, "`faces` must be a list of {body, point}"))?;
        for f in list {
            let body = str_(f, "body").ok_or_else(|| bad(cmd, "each face needs `body`"))?;
            let point = f.get("point").and_then(crate::params::vec3).ok_or_else(|| bad(cmd, "each face needs `point`: [x, y, z] on it"))?;
            let w = record(body)?;
            let end = w.end_at(point, 1e-2).ok_or_else(|| bad(cmd, format!("the point picked on `{body}` is not on one of its end faces")))?;
            ends.push(WeldEndRef { member: BodyName(body.to_string()), end: if end == 0 { WeldEndSide::Start } else { WeldEndSide::End } });
        }
    }
    if ends.is_empty() {
        return Err(bad(cmd, "`ends` (or `faces`) must pick the member ends to cap"));
    }
    if ends.len() > solidcraft_doc::MAX_WELD_MEMBERS * 2 {
        return Err(bad(cmd, format!("at most {} ends", solidcraft_doc::MAX_WELD_MEMBERS * 2)));
    }
    let thickness = length(s, p, "thickness", true, cmd)?.unwrap_or_default();
    let inset = length(s, p, "inset", false, cmd)?;
    let inside = flag(p, "inside", cmd)?;
    let corner = match str_(p, "corner").map(str::to_ascii_lowercase).as_deref() {
        None | Some("square" | "none") => WeldCapCorner::Square,
        Some("chamfer") => WeldCapCorner::Chamfer,
        Some("fillet") => WeldCapCorner::Fillet,
        _ => return Err(bad(cmd, "`corner` must be square, chamfer or fillet")),
    };
    let corner_size = length(s, p, "corner_size", !corner.is_square(), cmd)?;
    if corner.is_square() && corner_size.is_some() {
        return Err(bad(cmd, "`corner_size` needs `corner`: chamfer or fillet"));
    }
    let p = default_name(s, p, "End Cap");
    add_feature(s, &p, FeatureKind::WeldEndCap { ends, thickness, inset, inside, corner, corner_size })
}

fn gusset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.gusset";
    let st = s.model.state();
    let names = string_list(p, "members");
    let [a, b] = names.as_slice() else { return Err(bad(cmd, "`members` must name two structural members")) };
    for n in [a, b] {
        if !st.welds.iter().any(|w| &w.body == n) || st.body(n).is_none() {
            return Err(bad(cmd, format!("`{n}` is not a structural member")));
        }
    }
    if a == b {
        return Err(bad(cmd, "a gusset needs two different members"));
    }
    let members = [BodyName(a.clone()), BodyName(b.clone())];
    let size = length(s, p, "size", true, cmd)?.unwrap_or_default();
    let size2 = length(s, p, "size2", false, cmd)?;
    let thickness = length(s, p, "thickness", true, cmd)?.unwrap_or_default();
    let edge = length(s, p, "edge", false, cmd)?;
    let clip = length(s, p, "clip", false, cmd)?;
    let offset = length(s, p, "offset", false, cmd)?;
    let side = match str_(p, "side").map(str::to_ascii_lowercase).as_deref() {
        None | Some("center" | "centre") => WeldPlateSide::Center,
        Some("one") => WeldPlateSide::One,
        Some("other") => WeldPlateSide::Other,
        _ => return Err(bad(cmd, "`side` must be center, one or other")),
    };
    let reverse_first = flag(p, "reverse_first", cmd)?;
    let reverse_second = flag(p, "reverse_second", cmd)?;
    let p = default_name(s, p, "Gusset");
    add_feature(s, &p, FeatureKind::WeldGusset { members, size, size2, thickness, edge, clip, offset, side, reverse_first, reverse_second })
}

/// Longest weld note (characters).
const MAX_BEAD_NOTE: usize = 256;

/// Most points a weld bead's `at` takes.
const MAX_BEAD_POINTS: usize = 256;

fn bead(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.bead";
    let st = s.model.state();
    let names = string_list(p, "bodies");
    let [a, b] = names.as_slice() else { return Err(bad(cmd, "`bodies` must name the two bodies to weld")) };
    for n in [a, b] {
        if st.body(n).is_none() {
            return Err(bad(cmd, format!("no body `{n}`")));
        }
    }
    if a == b {
        return Err(bad(cmd, "a weld bead needs two different bodies"));
    }
    let bodies = [BodyName(a.clone()), BodyName(b.clone())];
    let size = length(s, p, "size", true, cmd)?.unwrap_or_default();
    let kind = match str_(p, "kind").map(str::to_ascii_lowercase).as_deref() {
        None | Some("fillet") => WeldBeadKind::Fillet,
        Some("groove") => WeldBeadKind::Groove,
        _ => return Err(bad(cmd, "`kind` must be fillet or groove")),
    };
    let at = match p.get("at") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(v)) if v.len() <= MAX_BEAD_POINTS => v
            .iter()
            .map(|x| crate::params::vec3(x).filter(|q| q.is_finite()).map(Position))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| bad(cmd, "`at` must list points [x, y, z] on the seams to weld"))?,
        Some(_) => return Err(bad(cmd, format!("`at` must list at most {MAX_BEAD_POINTS} points [x, y, z]"))),
    };
    let gap = length(s, p, "gap", false, cmd)?;
    let weld_length = length(s, p, "length", false, cmd)?;
    let pitch = length(s, p, "pitch", false, cmd)?;
    if weld_length.is_some() != pitch.is_some() {
        return Err(bad(cmd, "intermittent welds need both `length` and `pitch`"));
    }
    let field = flag(p, "field", cmd)?;
    let note = match p.get("note") {
        None | Some(Value::Null) => None,
        Some(Value::String(n)) if n.chars().count() <= MAX_BEAD_NOTE => Some(n.trim().to_string()).filter(|n| !n.is_empty()),
        Some(_) => return Err(bad(cmd, format!("`note` must be text of at most {MAX_BEAD_NOTE} characters"))),
    };
    add_feature(s, p, FeatureKind::WeldBead { bodies, size, kind, at, gap, length: weld_length, pitch, field, note })
}

/// A weld bead as listed: its feature, bodies, kind and size, the seams' and the welded length,
/// and (with `seams`) the polylines along them.
fn bead_json(b: &Bead, seams: bool) -> Value {
    let (welded, welds) = b.welded();
    let mut v = json!({"feature": b.feature, "name": b.name, "bodies": b.bodies, "kind": bead_kind(b), "size_mm": b.size,
        "seam_count": b.seams.len(), "seam_mm": b.length(), "length_mm": welded, "welds": welds,
        "intermittent": b.intermittent.map(|[l, p]| json!({"length_mm": l, "pitch_mm": p})), "field": b.field, "note": b.note,
        "volume_mm3": b.volume()});
    if seams {
        v["seams"] = json!(b.seams.iter().map(|s| s.iter().map(|q| [q.x, q.y, q.z]).collect::<Vec<_>>()).collect::<Vec<_>>());
    }
    v
}

fn bead_kind(b: &Bead) -> &'static str {
    match b.kind {
        WeldBeadKind::Fillet => "fillet",
        WeldBeadKind::Groove => "groove",
    }
}

/// The weld beads whose bodies are both still there.
fn live_beads(s: &Session) -> Vec<Bead> {
    let st = s.model.state();
    st.beads.iter().filter(|b| b.bodies.iter().all(|n| st.body(n).is_some())).cloned().collect()
}

fn beads(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.beads";
    let names = string_list(p, "bodies");
    let st = s.model.state();
    for n in &names {
        if st.body(n).is_none() {
            return Err(bad(cmd, format!("no body `{n}`")));
        }
    }
    let seams = flag_or(p, "seams", true, cmd)?;
    let list: Vec<Value> =
        live_beads(s).iter().filter(|b| names.is_empty() || b.bodies.iter().any(|n| names.contains(n))).map(|b| bead_json(b, seams)).collect();
    Ok(json!({"beads": list}))
}

/// The weld table: beads grouped where identical (same kind, size, welded length within `tol`,
/// intermittent spacing, field weld and note), each section's after the one before; a bead is
/// in its bodies' section when they share one, else in the weldment's own. With bodies asked
/// for, the beads on any of them; with a section, the beads in it.
fn weld_rows(s: &Session, only: &Only, tol: f64) -> Vec<Value> {
    struct Group {
        first: Bead,
        section: usize,
        names: Vec<String>,
        bodies: Vec<[String; 2]>,
        mass_g: f64,
    }
    let mut rows: Vec<Group> = Vec::new();
    for b in live_beads(s) {
        let [x, y] = &b.bodies;
        let section = if section(s, x) == section(s, y) { section(s, x) } else { 0 };
        let asked = only.bodies.is_empty() || b.bodies.iter().any(|n| only.bodies.contains(n));
        if !asked || only.section.is_some_and(|w| w != section) {
            continue;
        }
        let mass_g = b.volume().unwrap_or(0.0) * s.doc.density(x) / 1000.0;
        let same = |g: &Group| {
            let (f, o) = (&g.first, &b);
            g.section == section
                && f.kind == o.kind
                && (f.size - o.size).abs() <= tol
                && (f.welded().0 - o.welded().0).abs() <= tol
                && f.intermittent.map(|i| i.map(|v| (v / tol.max(1e-9)).round())) == o.intermittent.map(|i| i.map(|v| (v / tol.max(1e-9)).round()))
                && f.field == o.field
                && f.note == o.note
        };
        match rows.iter_mut().find(|g| same(g)) {
            Some(g) => {
                g.names.push(b.name.clone());
                g.bodies.push(b.bodies.clone());
                g.mass_g += mass_g;
            }
            None => rows.push(Group { names: vec![b.name.clone()], bodies: vec![b.bodies.clone()], first: b, section, mass_g }),
        }
    }
    rows.sort_by_key(|g| g.section);
    rows.iter()
        .enumerate()
        .map(|(i, g)| {
            let mut v = bead_json(&g.first, false);
            let qty = g.names.len().max(1);
            v["weld"] = json!(i + 1);
            v["qty"] = json!(qty);
            v["beads"] = json!(g.names);
            v["bodies"] = json!(g.bodies);
            v["mass_kg"] = json!(g.first.volume().map(|_| g.mass_g / 1000.0 / qty as f64));
            v["sub_weldment"] = section_name(s, g.section);
            for k in ["feature", "name"] {
                if let Some(o) = v.as_object_mut() {
                    o.remove(k);
                }
            }
            v
        })
        .collect()
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("weld.member", "Structural Member", member).at("WELDMENT", "CREATE").icon("sweep").params(
    "sketch: the profile's sketch; profiles? (one closed region, holes included; default: the sketch's only profile); \
     or stock (instead of sketch): a profile library profile's name (library? names the library file; see weld.profiles), \
     or {name, region} (a library profile as weld.member stores it): the profile is copied into the design; \
     path_sketch: the skeleton's sketch (2D or 3D); path: [ids of curves, one member each: a straight member along a \
     line, a bent one along an arc, one formed to shape along a spline, conic or other free-form curve or a 3D sketch curve \
     (an included model edge or a 3D curve whose points lie on one line or arc is that line or arc); a circle or other \
     curve closing on itself, smoothly, makes a ring, its seam where the curve starts]; merge_arcs?: bool (default true: \
     lines and curves that run on into one another, tangent where they meet with no other listed curve ending there, make \
     one bent member, keyed by the first of them listed, as tube is bent, a ring where they close on themselves; false: \
     each curve is a member of its own); end_allowance? (length of stock added at each end of a \
     bent member's cut length, trimmed off after bending: a roller's straight tails or a bender's grip; default 0); \
     angle? (the profile's turn about each line, about its located point); \
     locate?: centroid|top_left|top|top_right|left|middle|right|bottom_left|bottom|bottom_right|{point: [x, y]} (default centroid: \
     the point of the profile that lies on each line: its centroid, a point of its bounding box, or a point in the profile's own \
     coordinates, such as a corner, so that a face of the member runs along the line); \
     mirror?: none|left_right|up_down (default none: the profile mirrored about the vertical or horizontal axis through its \
     located point; the cut list knows a mirrored member as its stock turned end for end); corner?: mitre|butt (default mitre: how two members ending at one point are joined; \
     a butt's member running through is the one listed first in `path`); \
     groups?: [{path_sketch, path, angle?, corner?, locate?, mirror?}] (further groups of lines, each from its own sketch, with its \
     own angle, corner, located point and mirror, default the feature's; joined to `path` and to each other as if one path continued, where members of two groups meet \
     the later group's corner joins them, and a butt's earlier group runs through); \
     joints?: [{members: [key, key], corner: mitre|butt, through?: key}] (joints joined otherwise; a member's key is its line's id, \
     `N:id` for a line of group N, the groups numbered from 2); \
     gap? (length left between a butting member's end and the face it butts against); \
     join_earlier?: bool (default true: a member ending on a member of an earlier Structural Member in this component butts \
     against its face, the earlier member left as it is); \
     name?, body_name? (default Member: the first member; the others are numbered after it). Each member's name stays with \
     its skeleton line: editing `path` or `groups` (removing, reordering or adding lines or groups) renames no other member. \
     Where more than two members end at one point, the first two in `path` make \
     the corner and the others butt against the first; a member ending on another member's line butts against its face; \
     free ends are cut square. A bent member's ends are joined along its direction there, and a member ending on it (on a \
     straight run, a bend or a ring, of this feature or an earlier one) butts flat against the plane touching its face \
     there. A ring's seam is cut square. Not supported yet: a curve with a corner, a ring closing at a corner",
    ),
    CommandSpec::new("weld.corners", "Corners", corners).icon("list").noundo().params(
        "feature (a Structural Member: id or name), or path_sketch, path, groups?, corner?, joints?, merge_arcs? as weld.member takes them \
         → corners: [{members: [key, key], at: [x, y, z] (in the feature's component), corner: mitre|butt, through: key \
         (for a butt, the member running through) or null, own: bool (set by one of `joints`, else the feature's or \
         group's corner)}]. A corner is two members ending at one point (the first two in key order where more end there) \
         with no member of the feature running through it; its joint in `joints` changes how it is joined. Members of \
         earlier features are not looked at",
    ),
    CommandSpec::new("weld.trim_extend", "Trim/Extend", trim_extend).at("WELDMENT", "MODIFY").icon("trim").params(
        "members: [structural member bodies]; one boundary: plane (XY|XZ|YZ|plane name|{origin, normal}) | face: {body, point} \
         (a planar face: its plane; with cope, a round face) | to: [structural member bodies] (each member ends against the face, toward it, of the one \
         nearest its end); mode?: both|trim|extend (default both: cut back or lengthen as the boundary needs); gap? (left between \
         each end and the boundary); cope?: bool (with `to`: each end shaped to fit the outside of its boundary member, as \
         for a tube meeting a round tube; with a round `face` of any body, such as a pipe or tank: shaped to fit its cylinder, \
         `gap` larger; the cut list reads `coped`); name?. Each member's end nearest the boundary moves \
         onto it, cut flat on the boundary's plane (coped: through the boundary member's line or the face's axis, then the \
         outside taken away); the member's length, end cuts and line follow (cut list, later Structural Members). A member a \
         later feature changed can only be cut back. A bent member's end moves along its straight end run (lengthened \
         straight past a bend; cut back only within that run); a ring has no end. A boundary member may be bent: an end \
         meeting it on a bend is cut flat on the plane touching its face there, and coped only to a straight run. Cutting \
         flat to a curved face, and coping to a cone, sphere, torus or freeform face, are not supported yet",
    ),
    CommandSpec::new("weld.end_cap", "End Cap", end_cap).at("WELDMENT", "CREATE").icon("thicken").params(
        "ends: [{member: structural member body, end: start|end}] (or faces: [{body, point}] picking member end faces); \
         thickness; inset? (how far the plate's edge is set in from the profile's outside, or from its hole's side; default 0); \
         inside?: bool (default false: the plate stands on the end face; true: it is fitted into the profile's hole, flush with \
         the end); corner?: square|chamfer|fillet (default square) and corner_size (the chamfer's distance along each side, \
         or the fillet's radius): the plate's sharp outside corners between straight sides; name?, body_name? (default End Cap: the first cap; the others are numbered after it). One plate body per \
         end. An end cut at an angle takes a plate of its slanted face's shape, square-edged (the inset is measured square to \
         the member); a coped end fits against another member and takes none, and a ring has no open end",
    ),
    CommandSpec::new("weld.gusset", "Gusset", gusset).at("WELDMENT", "CREATE").icon("web").params(
        "members: [two structural member bodies whose lines meet]; size (leg length along the first member's face); size2? \
         (along the second's; default size); thickness; edge? (the plate's sides stand square off each member this far before \
         its slanted edge; default 0: a triangle); clip? (the corner at the joint cut off this far along each face, to clear \
         the weld; default 0); offset? (the plate moved off the plane of the two lines, toward its `one` side; default 0); \
         side?: center|one|other (default center: the thickness half each way; one: all toward the plane's normal, the first \
         leg's direction crossed with the second's; other: the other way); reverse_first?, reverse_second?: bool (that \
         member's leg runs the other way from the joint: the other corner where the member runs through it); name?, \
         body_name? (default Gusset). The plate lies in the corner between the two lines, its legs along the faces of the \
         members that face each other, each toward that member's far end (at a T, its longer part) unless reversed. On a \
         bent member the plate stands on the straight run that meets the other member (not on a bend, nor on a member \
         formed to a free-form shape)",
    ),
    CommandSpec::new("weld.bead", "Weld Bead", bead).at("WELDMENT", "CREATE").icon("fillet").params(
        "bodies: [the two bodies to weld: structural members, plates or any solid bodies]; size (a fillet's leg, or a groove \
         weld's throat); kind?: fillet|groove (default fillet); at?: [[x, y, z], …] (only the seam nearest each point; default \
         every seam the two bodies make); gap? (how far apart the bodies may stand at a seam: a root gap the weld bridges; \
         default 0, touching); length? and pitch? (intermittent welds: each this long, one every pitch along each seam, centre to \
         centre, the first at its start; both or neither); field?: bool (welded on site); note? (text for the weld symbol's tail: \
         a process or a specification); name?. Cosmetic: no material is added and no body changes; the seams are found where \
         the bodies meet (an edge of either with the other just across its face and the corner beside it open), not round a \
         hole in the face (a tube's bore) nor where no line of sight from outside the two bodies reaches. weld.beads lists them, \
         the cut list's welds table counts them",
    ),
    CommandSpec::new("weld.beads", "List Weld Beads", beads).icon("list").noundo().params(
        "bodies? (only the beads on any of these); seams?: bool (default true: each bead's seams as polylines) → beads: [{feature, \
         name, bodies, kind: fillet|groove, size_mm, seam_count, seam_mm (the seams' length), length_mm (welded: seam_mm, or for \
         intermittent welds the welds' length in all), welds (how many: one per seam, or per intermittent weld), intermittent: \
         {length_mm, pitch_mm} or null, field, note, volume_mm3 (a fillet's weld metal, legs size_mm; null for a groove weld), \
         seams: [[[x, y, z], …], …]}] (beads whose bodies are both still there)",
    ),
    CommandSpec::new("weld.cut_list", "Cut List", cut_list).at("WELDMENT", "INSPECT").icon("bendtable").noundo().params(
        "bodies? (default: every body); sub_weldment?: id or name (only its bodies); other_bodies?: bool (default true: list the bodies that are neither members nor plates too); length_tolerance? (mm, default 0.1) and angle_tolerance? (degrees, default 0.05): \
         members this close count as identical; kerf? (mm, default 3: the saw cut between pieces of one bar); mirrors?: separate (default: a member and its mirror image are different rows) | together \
         (one row, the mirror images listed in `mirrored`) → rows: [{item, qty, description (set by hand or by the library \
         profile, else the stock's name: the profile sketch's or the library profile's), length_mm (end to end at the long points; \
         a bent member's along its centreline), cut_length_mm (length_mm plus a bent member's end allowance at each end), \
         bends: [{feed_mm (straight fed before the bend: from the long point at the start, then from the last bend), rotation_deg \
         (the stock turned since the last bend), angle_deg, radius_mm (centreline)}] and tail_mm (straight after the last bend) \
         for a bent member (empty, and null: a straight member, or one formed to a free-form shape), formed: bool (formed to \
         a free-form shape: no bender steps), ring: bool (closes on itself: one seam), \
         end1, end2: {cut: square|mitre|compound, angle_deg | mitre_deg + bevel_deg}, mass_kg (each), material, members: [body names], \
         mirror_of: item whose members are this row's mirror images (or null), mirrored: [body names cut as mirror images of the first member]}], \
         profile (the stock's name), properties: {name: text}, sub_weldment: its name (or null)}], \
         stock: [{description (the stock's name), total_length_mm (of cut lengths), members, stock_lengths_mm (the lengths it is bought \
         in: weld.stock_lengths's, else its library profile's; empty: unknown, no bars counted), lengths_from: design|library|null, \
         kerf_mm, buy: [{length_mm, qty}] (bars to buy, longest first), bars: [{length_mm, items, cuts_mm, drop_mm}] (what each bar \
         is cut into: longest pieces first, each into the first bar with room, each bar bought as short as its pieces allow), \
         drop_mm (left over in all), too_long: [members longer than every length: to be spliced]}], members (count), plates: [{item (numbered on from the rows), qty, \
         kind: end_cap|gusset, thickness_mm, width_mm, height_mm (the outline's extent in the plate's frame, longer first), area_mm2, \
         mass_kg (each), material, bodies, description, properties}] (End Caps and Gussets; identical: same kind, material, \
         thickness and outline extent, area and length round within the length tolerance), plate_count, bodies: [{item (numbered on from the plates), \
         qty, kind: solid|sheet_metal, length_mm, width_mm, thickness_mm (the smallest bounding box, longest side first), volume_mm3, \
         mass_kg (each), material, bodies, description, properties}] (the other bodies; identical: same kind, material, box within the \
         length tolerance, volume within it times the surface area), body_count, excluded: [bodies left \
         out by weld.cut_properties], sub_weldments: [{id, name, bodies, items: [item numbers]}], welds: [{weld (numbered from 1), qty, \
         kind: fillet|groove, size_mm, seam_count, seam_mm, length_mm (welded, each), welds (each bead's count of welds), \
         intermittent: {length_mm, pitch_mm} or null, field, note, volume_mm3 and mass_kg (each: a fillet's weld metal at the first \
         body's density; null for a groove weld), beads: [bead names], bodies: [[two body names] per bead], sub_weldment}] (the \
         weld table: weld beads grouped where identical: same kind, size, welded length within the length tolerance, spacing, \
         field weld and note; with bodies asked for, the beads on any of them; a bead is in its bodies' sub-weldment when they \
         share one), weld_count (beads), weld_length_mm (welded in all). Each sub-weldment is a section of \
         its own: its rows (and plates and bodies rows, which carry `sub_weldment` too) hold only its bodies and follow the \
         weldment's own in each list, sub-weldments in the order they were made. A row's description and properties are its library profile's with the body's own \
         (weld.cut_properties) over them; rows hold bodies whose description and properties match. Identical members: same profile and material, \
         length within the tolerance, and each end's cut plane within the angle tolerance once one member is turned (or turned end for end, \
         as the profile's symmetry allows) to lie on the other",
    ),
    CommandSpec::new("weld.cut_properties", "Cut List Properties", cut_properties).icon("settings").doc_only().params(
        "bodies: [body names: structural members, plates or other bodies]; description?: text (empty or null: the library profile's again, or \
         the stock's name); properties?: {name: text, …} (free properties; empty or null removes the body's own, showing the \
         library profile's again); exclude?: bool (leave the bodies out of the cut list). All or nothing, one undo step → \
         bodies, changed: [keys]. Bodies whose description and properties differ are listed in different rows",
    ),
    CommandSpec::new("weld.stock_lengths", "Stock Lengths", set_stock_lengths).icon("settings").doc_only().params(
        "profile (a stock's name, as the cut list's stock lists it: a library profile's or a profile sketch's); lengths: [lengths \
         the stock is bought in, e.g. [\"20 ft\", \"40 ft\"]: bare numbers in the design's unit] (empty or null: the library \
         profile's again) → profile, lengths_mm. The design's own lengths, over the library profile's; the cut list counts the \
         bars to buy from them",
    ),
    CommandSpec::new("weld.sub_weldment", "Create Sub-weldment", sub_weldment).icon("folder").doc_only().params(
        "bodies: [body names: structural members, plates or other bodies]; name? (default Sub-weldment N; names are unique); \
         sub_weldment?: id or name (add the bodies to that one instead of a new one). The bodies leave any other sub-weldment, \
         and one left without bodies is removed → sub_weldment (id), name, bodies. A sub-weldment's bodies are listed in a \
         cut list section of their own",
    ),
    CommandSpec::new("weld.rename_sub_weldment", "Rename Sub-weldment", rename_sub_weldment).icon("rename").doc_only().params(
        "sub_weldment: id or name; name (unique) → sub_weldment, name",
    ),
    CommandSpec::new("weld.dissolve_sub_weldment", "Dissolve Sub-weldment", dissolve_sub_weldment).icon("folder").doc_only().params(
        "sub_weldment: id or name; bodies? (only these leave it; default all, which removes it) → sub_weldment, bodies (those that \
         left: listed with the weldment's own items again)",
    ),
    CommandSpec::new("weld.export_cut_list", "Export Cut List", export_cut_list).icon("bendtable").noundo().params(
        "format?: csv|json (default csv), path?; and weld.cut_list's bodies?, sub_weldment?, other_bodies?, length_tolerance?, angle_tolerance?, kerf?, mirrors? → the cut list as a file \
         (or its text, without a path): CSV columns item, qty, description, length_mm, end1, end1_mitre_deg, end1_bevel_deg, \
         end2, end2_mitre_deg, end2_bevel_deg, mass_kg, material, members (separated by `;`), mirror_of (empty for none), mirrored (separated by `;`), \
         profile, then a column per free property any row has; then, if there are plates, a blank line and a second table: item, qty, \
         plate (end_cap|gusset), thickness_mm, width_mm, height_mm, area_mm2, mass_kg, material, bodies (separated by `;`), description, \
         then a column per free property any plate has; then, if there are other bodies, another blank line and a third \
         table: item, qty, body (solid|sheet_metal), length_mm, width_mm, thickness_mm, volume_mm3, mass_kg, material, bodies \
         (separated by `;`), description, then a column per free property any of them has; then, where a stock's lengths are \
         known, another blank line and the bars: stock, bar (numbered per stock, or `too long` with the member in items), \
         length_mm, items and cuts_mm (separated by `;`), drop_mm; then, where there are weld beads, another blank line and the \
         weld table: weld, qty, kind, size_mm, length_mm, welds, intermittent_length_mm, pitch_mm, field, volume_mm3, mass_kg, \
         beads (separated by `;`), bodies (each bead's two joined by `+`, beads separated by `;`), note. With sub-weldments in the \
         list, each table but the bars has a sub_weldment column (the weld table's last, the others' before the property columns)",
    ),
];

fn corner(v: Option<&str>, cmd: &str) -> Result<WeldCorner> {
    match v {
        None | Some("mitre") => Ok(WeldCorner::Mitre),
        Some("butt") => Ok(WeldCorner::Butt),
        Some(other) => Err(bad(cmd, format!("corner `{other}` must be mitre or butt"))),
    }
}

/// `locate`: centroid, a bounding-box point, or `{point: [x, y]}` in the profile's coordinates.
fn locate(v: Option<&Value>, cmd: &str) -> Result<Option<WeldLocate>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(None) };
    let at: WeldLocate = serde_json::from_value(v.clone()).map_err(|_| {
        bad(
            cmd,
            "`locate` must be centroid, top_left, top, top_right, left, middle, right, bottom_left, bottom, bottom_right \
             or {point: [x, y]}",
        )
    })?;
    if let WeldLocate::Point(p) = at
        && !p.iter().all(|c| c.is_finite() && c.abs() <= solidcraft_doc::weld::MAX_STOCK_SIZE)
    {
        return Err(bad(cmd, "the `locate` point must be finite and within the profile's size limit"));
    }
    Ok(Some(at))
}

fn mirror(v: Option<&str>, cmd: &str) -> Result<Option<WeldMirror>> {
    match v {
        None => Ok(None),
        Some("none") => Ok(Some(WeldMirror::None)),
        Some("left_right") => Ok(Some(WeldMirror::LeftRight)),
        Some("up_down") => Ok(Some(WeldMirror::UpDown)),
        Some(other) => Err(bad(cmd, format!("mirror `{other}` must be none, left_right or up_down"))),
    }
}

fn groups(s: &Session, p: &Value, cmd: &str) -> Result<Vec<WeldGroup>> {
    let Some(v) = p.get("groups").filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let list = v.as_array().ok_or_else(|| bad(cmd, "`groups` must be a list"))?;
    if list.len() > solidcraft_doc::MAX_WELD_MEMBERS {
        return Err(bad(cmd, "too many groups"));
    }
    let mut out = Vec::with_capacity(list.len());
    for (g, n) in list.iter().zip(2..) {
        let path_sketch = sketch_id(s, g.get("path_sketch"), cmd, "path_sketch")?;
        let path = string_list(g, "path");
        if path.is_empty() {
            return Err(bad(cmd, format!("group {n}: `path` must list skeleton line ids")));
        }
        let angle = expr(g, "angle");
        if let Some(a) = &angle {
            check_expr(s, a, Kind::Angle, cmd, "angle")?;
        }
        let corner = str_(g, "corner").map(|c| corner(Some(c), cmd)).transpose()?;
        let (locate, mirror) = (locate(g.get("locate"), cmd)?, mirror(str_(g, "mirror"), cmd)?);
        out.push(WeldGroup { path_sketch: SketchId(path_sketch), path, angle, corner, locate, mirror });
    }
    Ok(out)
}

fn joints(p: &Value, path: &[String], cmd: &str) -> Result<Vec<WeldJoint>> {
    let Some(v) = p.get("joints") else { return Ok(Vec::new()) };
    let list = v.as_array().ok_or_else(|| bad(cmd, "`joints` must be a list"))?;
    if list.len() > solidcraft_doc::MAX_WELD_MEMBERS * 2 {
        return Err(bad(cmd, "too many joints"));
    }
    let mut out = Vec::with_capacity(list.len());
    for j in list {
        let members = string_list(j, "members");
        let [a, b] = members.as_slice() else { return Err(bad(cmd, "each joint needs `members`: two skeleton line ids")) };
        for m in [a, b] {
            if !path.contains(m) {
                return Err(bad(cmd, format!("joint member `{m}` is not a member's key (`path` line, or `N:line` of group N)")));
            }
        }
        if a == b {
            return Err(bad(cmd, "a joint needs two different members"));
        }
        let through = str_(j, "through").map(str::to_string);
        if let Some(t) = &through
            && t != a
            && t != b
        {
            return Err(bad(cmd, format!("joint `through` (`{t}`) must be one of its members")));
        }
        out.push(WeldJoint { members: [a.clone(), b.clone()], corner: corner(str_(j, "corner"), cmd)?, through });
    }
    Ok(out)
}

fn member(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.member";
    // The profile: a sketch's, or one from the profile library (by name, or as a copy).
    let stock = match p.get("stock").filter(|v| !v.is_null()) {
        Some(Value::String(name)) => Some(super::weld_library::find(p, name, cmd)?),
        Some(v) => {
            let stock: solidcraft_doc::WeldStock = serde_json::from_value(v.clone())
                .map_err(|e| bad(cmd, format!("`stock` must be a library profile's name or {{name, region}}: {e}")))?;
            solidcraft_doc::weld::check_stock(&stock.name, &stock.region).map_err(|e| bad(cmd, e))?;
            solidcraft_doc::weld::check_lengths(&stock.lengths).map_err(|e| bad(cmd, format!("`stock`: {e}")))?;
            Some(stock)
        }
        None => None,
    };
    let (sketch, profiles) = match (&stock, p.get("sketch").filter(|v| !v.is_null())) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give the profile as `sketch` or as `stock`, not both")),
        (Some(_), None) => (None, None),
        (None, v) => (Some(SketchId(sketch_id(s, v, cmd, "sketch")?)), Some(profiles(p, cmd)?)),
    };
    let path_sketch = sketch_id(s, p.get("path_sketch"), cmd, "path_sketch")?;
    let path = string_list(p, "path");
    if path.is_empty() {
        return Err(bad(cmd, "`path` must list skeleton line ids"));
    }
    let groups = groups(s, p, cmd)?;
    if path.len() + groups.iter().map(|g| g.path.len()).sum::<usize>() > solidcraft_doc::MAX_WELD_MEMBERS {
        return Err(bad(cmd, format!("a structural member takes at most {} lines", solidcraft_doc::MAX_WELD_MEMBERS)));
    }
    let angle = expr(p, "angle");
    if let Some(a) = &angle {
        check_expr(s, a, Kind::Angle, cmd, "angle")?;
    }
    let gap = expr(p, "gap");
    if let Some(g) = &gap {
        check_expr(s, g, Kind::Length, cmd, "gap")?;
    }
    let corner = corner(str_(p, "corner"), cmd)?;
    let (locate, mirror) = (locate(p.get("locate"), cmd)?.unwrap_or_default(), mirror(str_(p, "mirror"), cmd)?.unwrap_or_default());
    let keys = FeatureKind::WeldMember {
        sketch: None,
        profiles: None,
        stock: None,
        path_sketch: SketchId(path_sketch),
        path: path.clone(),
        angle: None,
        locate,
        mirror,
        corner,
        groups: groups.clone(),
        joints: Vec::new(),
        gap: None,
        join_earlier: true,
        merge_arcs: true,
        end_allowance: None,
    }
    .weld_keys();
    let joints = joints(p, &keys, cmd)?;
    let join_earlier = flag_or(p, "join_earlier", true, cmd)?;
    let merge_arcs = flag_or(p, "merge_arcs", true, cmd)?;
    let end_allowance = expr(p, "end_allowance");
    if let Some(a) = &end_allowance {
        check_expr(s, a, Kind::Length, cmd, "end_allowance")?;
    }
    // Members are named after the first, so every line's name is stored with the feature.
    let mut p = p.clone();
    if str_(&p, "body_name").is_none() && string_list(&p, "body_names").is_empty() {
        p["body_name"] = json!(free_name(s, "Member"));
    }
    add_feature(
        s,
        &p,
        FeatureKind::WeldMember {
            sketch,
            profiles,
            stock,
            path_sketch: SketchId(path_sketch),
            path,
            angle,
            locate,
            mirror,
            corner,
            groups,
            joints,
            gap,
            join_earlier,
            merge_arcs,
            end_allowance,
        },
    )
}

fn corners(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.corners";
    let (kind, component) = match p.get("feature").filter(|v| !v.is_null()) {
        Some(v) => {
            let key = match v {
                Value::Number(n) => n.to_string(),
                Value::String(x) => x.clone(),
                _ => return Err(bad(cmd, "`feature` must be an id or name")),
            };
            let f = s.doc.find_feature(&key).ok_or_else(|| bad(cmd, format!("no feature `{key}`")))?;
            if !matches!(f.kind, FeatureKind::WeldMember { .. }) {
                return Err(bad(cmd, format!("`{key}` is not a Structural Member")));
            }
            (f.kind.clone(), f.component)
        }
        None => {
            let path_sketch = sketch_id(s, p.get("path_sketch"), cmd, "path_sketch")?;
            let path = string_list(p, "path");
            if path.is_empty() {
                return Err(bad(cmd, "give `feature`, or `path_sketch` and `path` as for weld.member"));
            }
            let groups = groups(s, p, cmd)?;
            if path.len() + groups.iter().map(|g| g.path.len()).sum::<usize>() > solidcraft_doc::MAX_WELD_MEMBERS {
                return Err(bad(cmd, format!("a structural member takes at most {} lines", solidcraft_doc::MAX_WELD_MEMBERS)));
            }
            let mut kind = FeatureKind::WeldMember {
                sketch: None,
                profiles: None,
                stock: None,
                path_sketch: SketchId(path_sketch),
                path,
                angle: None,
                locate: WeldLocate::default(),
                mirror: WeldMirror::default(),
                corner: corner(str_(p, "corner"), cmd)?,
                groups,
                joints: Vec::new(),
                gap: None,
                join_earlier: true,
                merge_arcs: flag_or(p, "merge_arcs", true, cmd)?,
                end_allowance: None,
            };
            let keys = kind.weld_keys();
            if let FeatureKind::WeldMember { joints: j, .. } = &mut kind {
                *j = joints(p, &keys, cmd)?;
            }
            (kind, s.active_component)
        }
    };
    let found = solidcraft_doc::weld_corners(&s.doc, &s.model.state(), &kind, component).map_err(|e| bad(cmd, e.to_string()))?;
    let corners: Vec<Value> = found
        .iter()
        .map(|c| {
            json!({
                "members": c.members,
                "at": [c.at.x, c.at.y, c.at.z],
                "corner": if c.corner == WeldCorner::Butt { "butt" } else { "mitre" },
                "through": c.through,
                "own": c.own,
            })
        })
        .collect();
    Ok(json!({"corners": corners}))
}

/// `base`, or `base (2)`, `base (3)`… if a body already has that name.
fn free_name(s: &Session, base: &str) -> String {
    let st = s.model.state();
    std::iter::once(base.to_string()).chain((2..).map(|i| format!("{base} ({i})"))).find(|n| st.body(n).is_none()).unwrap_or_default()
}

/// A Structural Member edited from `old` to `new`: where its skeleton lines change, its body names
/// by line ([`names_by_line`]) as its members take them (a bent member of several lines its first
/// line's), and with `rekey`, `new`'s joints (still `old`'s) moved to their members' new keys, a
/// joint of a member no longer there dropped. `None` where the lines are the same.
pub(super) fn follow_lines(s: &Session, old: &solidcraft_doc::Feature, new: &mut FeatureKind, rekey: bool) -> Option<Vec<String>> {
    let (was, now) = (member_lines(&old.kind), member_lines(new));
    if was == now || now.is_empty() {
        return None;
    }
    let by_line = names_by_line(s, old, &was, &old.kind.weld_keys(), &now);
    let names = match solidcraft_doc::weld_member_keys(&s.doc, &s.model.state(), new, old.component) {
        Ok(firsts) => firsts.iter().filter_map(|&i| by_line.get(i).cloned()).collect(),
        Err(_) => by_line,
    };
    if rekey {
        let joints = solidcraft_doc::weld::rekey_joints(&old.kind, &now, &new.weld_keys());
        if let FeatureKind::WeldMember { joints: j, .. } = new {
            *j = joints;
        }
    }
    Some(names)
}

/// A Structural Member's body names after its skeleton lines are edited from `was` (keyed
/// `old_keys`) to `now`: each line keeps the name its member has now (or had stored, where the
/// feature doesn't build), and lines new to it are numbered after its first name, past every
/// number in use.
fn names_by_line(s: &Session, old: &solidcraft_doc::Feature, was: &[(SketchId, &str)], old_keys: &[String], now: &[(SketchId, &str)]) -> Vec<String> {
    let made = s.model.result(old.id).map(|r| r.state.clone());
    // The stored names follow the members; a member goes by its first line's key.
    let old_firsts = solidcraft_doc::weld_member_keys(&s.doc, &s.model.state(), &old.kind, old.component).ok();
    let current = |line: &(SketchId, &str)| -> Option<String> {
        let i = was.iter().position(|l| l == line)?;
        let key = old_keys.get(i)?;
        let stored = match &old_firsts {
            Some(firsts) => firsts.iter().position(|&f| f == i)?,
            None => i,
        };
        made.as_ref()
            .and_then(|st| st.welds.iter().find(|w| w.feature == old.id && &w.line == key).map(|w| w.body.clone()))
            .or_else(|| old.body_names.get(stored).filter(|n| !n.trim().is_empty()).cloned())
    };
    let mut names: Vec<Option<String>> = now.iter().map(current).collect();
    let first = names.iter().flatten().next().or(old.body_names.first()).map_or("Member", String::as_str);
    // `Rail (3)` is numbered after `Rail`.
    let base = match first.rsplit_once(" (") {
        Some((b, n)) if n.strip_suffix(')').is_some_and(|n| n.parse::<u32>().is_ok()) => b,
        _ => first,
    }
    .to_string();
    let st = s.model.state();
    let mut taken: std::collections::BTreeSet<String> = names.iter().flatten().cloned().collect();
    taken.extend(old.body_names.iter().cloned());
    let mut next = 2u32;
    for slot in names.iter_mut().filter(|n| n.is_none()) {
        let name = loop {
            let n = format!("{base} ({next})");
            next = next.saturating_add(1);
            // The feature's own bodies are rebuilt; another feature's keep their names.
            let other = st.body(&n).is_some_and(|b| b.feature != old.id);
            if !taken.contains(&n) && !other {
                break n;
            }
        };
        taken.insert(name.clone());
        *slot = Some(name);
    }
    names.into_iter().flatten().collect()
}

/// A body's cut list description (empty: the stock's name) and free properties: its library
/// profile's, with its own (`weld.cut_properties`) over them.
#[derive(Clone, Default, PartialEq)]
struct Props {
    description: String,
    properties: BTreeMap<String, String>,
}

fn props_of(s: &Session, body: &str, feature: u64) -> Props {
    let mut out = match s.doc.feature(feature).map(|f| &f.kind) {
        Some(FeatureKind::WeldMember { stock: Some(stock), .. }) => {
            Props { description: stock.description.clone(), properties: stock.properties.clone() }
        }
        _ => Props::default(),
    };
    if let Some(own) = s.doc.cut_list.get(body) {
        if !own.description.is_empty() {
            out.description = own.description.clone();
        }
        out.properties.extend(own.properties.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    out
}

/// Whether the cut list can name `body`: a body of the design, or a member or plate recorded
/// for it (one a later feature consumed still takes properties).
fn listable(s: &Session, body: &str) -> bool {
    let st = s.model.state();
    st.body(body).is_some() || st.welds.iter().any(|w| w.body == body) || st.plates.iter().any(|w| w.body == body)
}

fn excluded(s: &Session, body: &str) -> bool {
    s.doc.cut_list.get(body).is_some_and(|c| c.exclude)
}

/// The cut list section `body` is listed in: 0 for the weldment's own items, else one past its
/// sub-weldment's index.
fn section(s: &Session, body: &str) -> usize {
    s.doc.sub_weldments.iter().position(|w| w.bodies.iter().any(|b| b == body)).map_or(0, |i| i + 1)
}

/// The name of a cut list section's sub-weldment (null for the weldment's own items).
fn section_name(s: &Session, section: usize) -> Value {
    section.checked_sub(1).and_then(|i| s.doc.sub_weldments.get(i)).map_or(Value::Null, |w| json!(w.name))
}

/// The index of the sub-weldment `p[key]` names, by id or name.
fn sub_weldment_index(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<usize> {
    let found = match p.get(key) {
        Some(Value::Number(n)) => n.as_u64().and_then(|id| s.doc.sub_weldments.iter().position(|w| w.id == id)),
        Some(Value::String(name)) => s.doc.sub_weldments.iter().position(|w| w.name == *name),
        _ => return Err(bad(cmd, format!("`{key}` must be a sub-weldment id or name"))),
    };
    found.ok_or_else(|| bad(cmd, format!("no sub-weldment {}", p[key])))
}

/// Which bodies a cut list lists: those named (all when none are) in the one section asked for
/// (all sections when none is).
struct Only {
    bodies: Vec<String>,
    section: Option<usize>,
}

impl Only {
    fn keeps(&self, s: &Session, body: &str) -> bool {
        (self.bodies.is_empty() || self.bodies.iter().any(|b| b == body)) && self.section.is_none_or(|x| section(s, body) == x)
    }
}

/// One row of the cut list: identical members.
struct Row<'a> {
    first: &'a MemberCut,
    section: usize,
    props: Props,
    material: String,
    mass_g: f64,
    members: Vec<&'a str>,
    /// With `mirrors: together`, the members cut as the first member's mirror image.
    mirrored: Vec<&'a str>,
}

fn tolerance(p: &Value, key: &str, default: f64, cmd: &str) -> Result<f64> {
    match num(p, key) {
        None if p.get(key).is_none() => Ok(default),
        Some(t) if t.is_finite() && (0.0..=1000.0).contains(&t) => Ok(t),
        _ => Err(bad(cmd, format!("`{key}` must be a number from 0 to 1000"))),
    }
}

fn end_json(e: &MemberEnd) -> Value {
    match *e {
        MemberEnd::Square => json!({"cut": "square"}),
        MemberEnd::Mitre { angle } => json!({"cut": "mitre", "angle_deg": angle}),
        MemberEnd::Compound { mitre, bevel } => json!({"cut": "compound", "mitre_deg": mitre, "bevel_deg": bevel}),
        MemberEnd::Coped => json!({"cut": "coped"}),
    }
}

/// The cut list as JSON: its rows in the order their first members were made, the stock each
/// profile needs, and the member count.
/// Bodies the cut list measured lately, by shape (a recoloured body keeps its entry): listing
/// again after an edit, or exporting what was just listed, measures only what changed.
static MEASURED: std::sync::Mutex<Vec<(solidcraft_kernel::Body, solidcraft_kernel::MassMeasure)>> = std::sync::Mutex::new(Vec::new());
const MEASURED_KEPT: usize = 256;

/// The mass properties of body `name` (tessellated: for a body the cut list can't work out
/// from how it was made).
fn mass_of(body: &solidcraft_kernel::Body, name: &str) -> Result<solidcraft_kernel::MassMeasure> {
    if let Ok(mut kept) = MEASURED.lock()
        && let Some(i) = kept.iter().position(|(b, _)| b.same_shape(body))
    {
        let hit = kept.remove(i);
        let m = hit.1;
        kept.insert(0, hit);
        return Ok(m);
    }
    let m = solidcraft_kernel::measure_mass(body).map_err(|e| EngineError::Other(format!("{name}: {e}")))?;
    if let Ok(mut kept) = MEASURED.lock() {
        kept.insert(0, (body.clone(), m));
        kept.truncate(MEASURED_KEPT);
    }
    Ok(m)
}

/// The saw kerf (mm) the cut list leaves between pieces cut from one bar, where not given.
const DEFAULT_KERF: f64 = 3.0;

/// One stock of the cut list: its name, the total of its cut lengths, its pieces (cut length,
/// item, body) and the lengths it is bought in (longest first), from the design or the library.
struct Stock<'a> {
    name: String,
    total: f64,
    pieces: Vec<(f64, usize, &'a str)>,
    lengths: Vec<f64>,
    from: Option<&'static str>,
}

/// One bar of stock and the pieces (index into the stock's) cut from it.
struct Bar {
    length: f64,
    used: f64,
    pieces: Vec<usize>,
}

/// The bars to buy for `pieces`, cut `kerf` apart, from bars of `lengths` (longest first): the
/// longest pieces first, each into the first bar it fits (a new bar of the longest length when
/// none has room), and each bar then bought in the shortest length its pieces fit. Pieces longer
/// than every length are left out (the second list).
fn cut_bars(pieces: &[(f64, usize, &str)], lengths: &[f64], kerf: f64) -> (Vec<Bar>, Vec<usize>) {
    let Some(&longest) = lengths.first() else { return (Vec::new(), Vec::new()) };
    let mut order: Vec<(usize, f64)> = pieces.iter().map(|p| p.0).enumerate().collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut bars: Vec<Bar> = Vec::new();
    let mut too_long = Vec::new();
    for (i, len) in order {
        if len > longest + 1e-6 {
            too_long.push(i);
            continue;
        }
        let room = |b: &Bar| b.used + kerf + len <= b.length + 1e-6;
        match bars.iter_mut().find(|b| room(b)) {
            Some(b) => {
                b.used += kerf + len;
                b.pieces.push(i);
            }
            None => bars.push(Bar { length: longest, used: len, pieces: vec![i] }),
        }
    }
    for b in &mut bars {
        if let Some(&l) = lengths.iter().rev().find(|&&l| b.used <= l + 1e-6) {
            b.length = l;
        }
    }
    too_long.sort_unstable();
    (bars, too_long)
}

/// A stock of the cut list as JSON, with the bars to buy where its lengths are known.
fn stock_json(x: &Stock, kerf: f64) -> Value {
    let (bars, too_long) = cut_bars(&x.pieces, &x.lengths, kerf);
    let mut buy: Vec<(f64, usize)> = Vec::new();
    for b in &bars {
        match buy.iter_mut().find(|(l, _)| (*l - b.length).abs() < 1e-6) {
            Some(n) => n.1 += 1,
            None => buy.push((b.length, 1)),
        }
    }
    buy.sort_by(|a, b| b.0.total_cmp(&a.0));
    let drop = bars.iter().fold(0.0, |d, b| d + (b.length - b.used));
    let bars: Vec<Value> = bars
        .iter()
        .map(|b| {
            let items: Vec<usize> = b.pieces.iter().filter_map(|&i| x.pieces.get(i)).map(|p| p.1).collect();
            let cuts: Vec<f64> = b.pieces.iter().filter_map(|&i| x.pieces.get(i)).map(|p| p.0).collect();
            json!({"length_mm": b.length, "items": items, "cuts_mm": cuts, "drop_mm": b.length - b.used})
        })
        .collect();
    let too_long: Vec<&str> = too_long.iter().filter_map(|&i| x.pieces.get(i)).map(|p| p.2).collect();
    json!({"description": x.name, "total_length_mm": x.total, "members": x.pieces.len(), "stock_lengths_mm": x.lengths,
        "lengths_from": x.from, "kerf_mm": kerf,
        "buy": buy.iter().map(|(l, n)| json!({"length_mm": l, "qty": n})).collect::<Vec<_>>(),
        "bars": bars, "drop_mm": drop, "too_long": too_long})
}

fn cut_list_json(s: &Session, p: &Value, cmd: &str) -> Result<Value> {
    let length_tol = tolerance(p, "length_tolerance", 0.1, cmd)?;
    let angle_tol = tolerance(p, "angle_tolerance", 0.05, cmd)?;
    let together = match str_(p, "mirrors").map(str::to_ascii_lowercase).as_deref() {
        None | Some("separate") => false,
        Some("together") => true,
        _ => return Err(bad(cmd, "`mirrors` must be separate or together")),
    };
    let only = Only {
        bodies: string_list(p, "bodies"),
        section: if p.get("sub_weldment").is_some() { Some(sub_weldment_index(s, p, "sub_weldment", cmd)? + 1) } else { None },
    };
    let st = s.model.state();
    for b in &only.bodies {
        if !listable(s, b) {
            return Err(bad(cmd, format!("no body `{b}`")));
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut left_out: Vec<String> = Vec::new();
    for w in st.welds.iter().filter(|w| only.keeps(s, &w.body)) {
        // A member a later feature consumed (combined into another body) is no longer cut.
        let Some(body) = st.body(&w.body) else { continue };
        if excluded(s, &w.body) {
            left_out.push(w.body.clone());
            continue;
        }
        let props = props_of(s, &w.body, w.feature);
        let section = section(s, &w.body);
        let volume = match w.volume(&body.body) {
            Some(v) => v,
            None => mass_of(&body.body, &w.body)?.volume,
        };
        let mass_g = volume * s.doc.density(&w.body) / 1000.0;
        let material = s.doc.materials.get(&w.body).cloned().unwrap_or_else(|| "Default".into());
        let like = |r: &Row| {
            if r.section == section
                && r.first.profile == w.profile
                && r.material == material
                && r.props == props
                && (r.first.length - w.length).abs() <= length_tol
                && (r.first.cut_length() - w.cut_length()).abs() <= length_tol
                && same_bends(r.first, w, length_tol, angle_tol)
            {
                likeness(r.first, w, angle_tol)
            } else {
                Likeness::Different
            }
        };
        let row = rows.iter_mut().find_map(|r| match like(r) {
            Likeness::Same => Some((r, false)),
            Likeness::Mirror if together => Some((r, true)),
            _ => None,
        });
        match row {
            Some((r, mirror)) => {
                r.mass_g += mass_g;
                r.members.push(&w.body);
                if mirror {
                    r.mirrored.push(&w.body);
                }
            }
            None => rows.push(Row { first: w, section, props, material, mass_g, members: vec![&w.body], mirrored: Vec::new() }),
        }
    }
    // Each section's rows together, the weldment's own first.
    rows.sort_by_key(|r| r.section);
    // Kept apart, each mirror-image row names its partner.
    let mirror_of: Vec<Option<usize>> = rows
        .iter()
        .map(|r| {
            rows.iter().position(|o| {
                !std::ptr::eq(o, r)
                    && o.section == r.section
                    && o.first.profile == r.first.profile
                    && o.material == r.material
                    && o.props == r.props
                    && (o.first.length - r.first.length).abs() <= length_tol
                    && (o.first.cut_length() - r.first.cut_length()).abs() <= length_tol
                    && same_bends(o.first, r.first, length_tol, angle_tol)
                    && likeness(o.first, r.first, angle_tol) == Likeness::Mirror
            })
        })
        .collect();
    let kerf = match num(p, "kerf") {
        None if p.get("kerf").is_none_or(Value::is_null) => DEFAULT_KERF,
        Some(k) if k.is_finite() && (0.0..=100.0).contains(&k) => k,
        _ => return Err(bad(cmd, "`kerf` must be a number of mm from 0 to 100")),
    };
    // Each stock: its total, member count and pieces (cut length, item, body), and where its
    // lengths come from: the design's own, else the first library profile giving some.
    let mut stock: Vec<Stock> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let j = match stock.iter().position(|x| x.name == r.first.profile) {
            Some(j) => j,
            None => {
                stock.push(Stock { name: r.first.profile.clone(), total: 0.0, pieces: Vec::new(), lengths: Vec::new(), from: None });
                stock.len() - 1
            }
        };
        let Some(x) = stock.get_mut(j) else { continue };
        x.total += r.first.cut_length() * r.members.len() as f64;
        x.pieces.extend(r.members.iter().map(|b| (r.first.cut_length(), i + 1, *b)));
        if x.from.is_none() {
            if let Some(own) = s.doc.stock_lengths.get(&x.name).filter(|l| !l.is_empty()) {
                x.lengths = own.clone();
                x.from = Some("design");
            } else if let Some(FeatureKind::WeldMember { stock: Some(st), .. }) = s.doc.feature(r.first.feature).map(|f| &f.kind)
                && !st.lengths.is_empty()
            {
                x.lengths = st.lengths.clone();
                x.from = Some("library");
            }
        }
    }
    let members: usize = rows.iter().map(|r| r.members.len()).sum();
    let rows: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let description = if r.props.description.is_empty() { &r.first.profile } else { &r.props.description };
            let (bends, tail) = match &r.first.bent {
                // Formed to a free-form shape: no bender steps.
                Some(b) if b.formed => (Vec::new(), Value::Null),
                Some(b) => {
                    let (steps, tail) = bend_steps(b);
                    let steps: Vec<Value> = steps
                        .iter()
                        .map(|b| json!({"feed_mm": b.feed, "rotation_deg": b.rotation, "angle_deg": b.angle, "radius_mm": b.radius}))
                        .collect();
                    (steps, json!(tail))
                }
                None => (Vec::new(), Value::Null),
            };
            json!({"item": i + 1, "qty": r.members.len(), "description": description, "length_mm": r.first.length,
                "cut_length_mm": r.first.cut_length(), "bends": bends, "tail_mm": tail,
                "formed": r.first.bent.as_ref().is_some_and(|b| b.formed), "ring": r.first.is_ring(),
                "end1": end_json(&r.first.ends[0]), "end2": end_json(&r.first.ends[1]),
                "mass_kg": r.mass_g / 1000.0 / r.members.len().max(1) as f64, "material": r.material, "members": r.members,
                "mirror_of": mirror_of.get(i).copied().flatten().map(|j| j + 1), "mirrored": r.mirrored,
                "profile": r.first.profile, "properties": r.props.properties, "sub_weldment": section_name(s, r.section)})
        })
        .collect();
    let stock: Vec<Value> = stock.iter().map(|x| stock_json(x, kerf)).collect();
    let plates = plate_rows(s, &only, length_tol, rows.len(), &mut left_out)?;
    let plate_count: usize = plates.iter().filter_map(|r| r["qty"].as_u64()).sum::<u64>() as usize;
    let others = match p.get("other_bodies") {
        None => true,
        Some(_) => bool_(p, "other_bodies").ok_or_else(|| bad(cmd, "`other_bodies` must be true or false"))?,
    };
    let bodies = if others { body_rows(s, &only, length_tol, rows.len() + plates.len(), &mut left_out)? } else { Vec::new() };
    let body_count: usize = bodies.iter().filter_map(|r| r["qty"].as_u64()).sum::<u64>() as usize;
    // The sub-weldments listed, with the items of their sections.
    let sub_weldments: Vec<Value> = s
        .doc
        .sub_weldments
        .iter()
        .enumerate()
        .filter(|(i, _)| only.section.is_none_or(|x| x == i + 1))
        .map(|(_, w)| {
            let items: Vec<&Value> =
                rows.iter().chain(&plates).chain(&bodies).filter(|r| r["sub_weldment"] == w.name.as_str()).map(|r| &r["item"]).collect();
            json!({"id": w.id, "name": w.name, "bodies": w.bodies, "items": items})
        })
        .collect();
    let welds = weld_rows(s, &only, length_tol);
    let weld_count: usize = welds.iter().filter_map(|r| r["qty"].as_u64()).sum::<u64>() as usize;
    let weld_length: f64 = welds.iter().map(|r| r["length_mm"].as_f64().unwrap_or(0.0) * r["qty"].as_f64().unwrap_or(0.0)).sum();
    Ok(json!({"rows": rows, "stock": stock, "members": members, "plates": plates, "plate_count": plate_count, "bodies": bodies,
        "body_count": body_count, "excluded": left_out, "sub_weldments": sub_weldments, "welds": welds, "weld_count": weld_count,
        "weld_length_mm": weld_length}))
}

/// The design's other solid bodies (neither a structural member nor a plate) grouped where
/// identical (same kind, material, description and properties, bounding box within `tol` and
/// volume within `tol` all over their surface), numbered on from `items`; bodies left out go to
/// `left_out`.
fn body_rows(s: &Session, only: &Only, tol: f64, items: usize, left_out: &mut Vec<String>) -> Result<Vec<Value>> {
    struct Other<'a> {
        name: &'a str,
        section: usize,
        sheet: bool,
        size: [f64; 3],
        volume: f64,
        area: f64,
        material: String,
        props: Props,
    }
    let st = s.model.state();
    let mut rows: Vec<(Other, f64, Vec<&str>)> = Vec::new();
    for b in st.bodies.iter().filter(|b| only.keeps(s, &b.name)) {
        if st.welds.iter().any(|w| w.body == b.name) || st.plates.iter().any(|p| p.body == b.name) {
            continue;
        }
        if excluded(s, &b.name) {
            left_out.push(b.name.clone());
            continue;
        }
        let m = mass_of(&b.body, &b.name)?;
        let mesh = b.mesh();
        let other = Other {
            name: &b.name,
            section: section(s, &b.name),
            sheet: st.sheets.iter().any(|x| x.body == b.name),
            size: solidcraft_doc::weld::body_box(&mesh.positions, &mesh.triangles),
            volume: m.volume,
            area: m.area,
            material: s.doc.materials.get(&b.name).cloned().unwrap_or_else(|| "Default".into()),
            props: props_of(s, &b.name, b.feature),
        };
        let mass_g = m.volume * s.doc.density(&b.name) / 1000.0;
        let same = |r: &Other| {
            r.section == other.section
                && r.sheet == other.sheet
                && r.material == other.material
                && r.props == other.props
                && r.size.iter().zip(other.size).all(|(a, b)| (a - b).abs() <= tol)
                && (r.volume - other.volume).abs() <= tol * r.area.max(other.area).max(1.0)
        };
        match rows.iter_mut().find(|r| same(&r.0)) {
            Some(r) => {
                r.1 += mass_g;
                r.2.push(&b.name);
            }
            None => {
                let name = other.name;
                rows.push((other, mass_g, vec![name]));
            }
        }
    }
    rows.sort_by_key(|r| r.0.section);
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, (b, mass_g, bodies))| {
            json!({"item": items + i + 1, "qty": bodies.len(), "kind": if b.sheet { "sheet_metal" } else { "solid" },
                "length_mm": b.size[0], "width_mm": b.size[1], "thickness_mm": b.size[2], "volume_mm3": b.volume,
                "mass_kg": mass_g / 1000.0 / bodies.len().max(1) as f64, "material": b.material, "bodies": bodies,
                "description": b.props.description, "properties": b.props.properties, "sub_weldment": section_name(s, b.section)})
        })
        .collect())
}

/// The plates (End Caps, Gussets) grouped where identical (same kind, material, thickness and
/// outline, within `tol`, and the same description and properties), numbered on from the
/// members' `items` rows; plates left out of the cut list go to `left_out`.
fn plate_rows(s: &Session, only: &Only, tol: f64, items: usize, left_out: &mut Vec<String>) -> Result<Vec<Value>> {
    let st = s.model.state();
    let mut rows: Vec<(&PlateCut, String, f64, Vec<&str>, Props, usize)> = Vec::new();
    for p in st.plates.iter().filter(|p| only.keeps(s, &p.body)) {
        let Some(body) = st.body(&p.body) else { continue };
        if excluded(s, &p.body) {
            left_out.push(p.body.clone());
            continue;
        }
        let props = props_of(s, &p.body, p.feature);
        let section = section(s, &p.body);
        let volume = match p.volume(&body.body) {
            Some(v) => v,
            None => mass_of(&body.body, &p.body)?.volume,
        };
        let mass_g = volume * s.doc.density(&p.body) / 1000.0;
        let material = s.doc.materials.get(&p.body).cloned().unwrap_or_else(|| "Default".into());
        match rows.iter_mut().find(|r| r.5 == section && r.1 == material && r.4 == props && r.0.same(p, tol)) {
            Some(r) => {
                r.2 += mass_g;
                r.3.push(&p.body);
            }
            None => rows.push((p, material, mass_g, vec![&p.body], props, section)),
        }
    }
    rows.sort_by_key(|r| r.5);
    Ok(rows
        .iter()
        .enumerate()
        .map(|(i, (p, material, mass_g, bodies, props, section))| {
            let kind = match p.kind {
                PlateKind::EndCap => "end_cap",
                PlateKind::Gusset => "gusset",
            };
            json!({"item": items + i + 1, "qty": bodies.len(), "kind": kind, "thickness_mm": p.thickness, "width_mm": p.size[0],
                "height_mm": p.size[1], "area_mm2": p.area, "mass_kg": mass_g / 1000.0 / bodies.len().max(1) as f64,
                "material": material, "bodies": bodies, "description": props.description, "properties": props.properties,
                "sub_weldment": section_name(s, *section)})
        })
        .collect())
}

fn cut_properties(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.cut_properties";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list body names"));
    }
    for b in &bodies {
        if !listable(s, b) {
            return Err(bad(cmd, format!("no body `{b}`")));
        }
    }
    let description = p.get("description").map(|v| property("description", v, cmd)).transpose()?;
    let properties: Vec<(String, Option<String>)> = match p.get("properties") {
        None => Vec::new(),
        Some(Value::Object(v)) if v.len() <= MAX_KEYS => v.iter().map(|(k, v)| Ok((k.clone(), property(k, v, cmd)?))).collect::<Result<_>>()?,
        Some(Value::Object(_)) => return Err(bad(cmd, format!("`properties` can hold at most {MAX_KEYS} names"))),
        Some(_) => return Err(bad(cmd, "`properties` must be an object: {name: text, …}")),
    };
    let exclude = match p.get("exclude") {
        None => None,
        Some(_) => Some(bool_(p, "exclude").ok_or_else(|| bad(cmd, "`exclude` must be true or false"))?),
    };
    if description.is_none() && properties.is_empty() && exclude.is_none() {
        return Err(bad(cmd, "give description, properties or exclude"));
    }
    let mut next = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    for b in &bodies {
        let was = s.doc.cut_list.get(b).cloned().unwrap_or_default();
        let mut now = was.clone();
        if let Some(d) = &description {
            now.description = d.clone().unwrap_or_default();
        }
        for (name, value) in &properties {
            match value {
                Some(text) => now.properties.insert(name.clone(), text.clone()),
                None => now.properties.remove(name),
            };
        }
        if now.properties.len() > MAX_KEYS {
            return Err(bad(cmd, format!("a body holds at most {MAX_KEYS} cut list properties")));
        }
        if let Some(x) = exclude {
            now.exclude = x;
        }
        let mut note = |key: String| {
            if !changed.contains(&key) {
                changed.push(key);
            }
        };
        if now.description != was.description {
            note("description".into());
        }
        for (name, _) in &properties {
            if now.properties.get(name) != was.properties.get(name) {
                note(format!("properties.{name}"));
            }
        }
        if now.exclude != was.exclude {
            note("exclude".into());
        }
        next.push((b.clone(), now));
    }
    // Only a real change touches the design (an undo step, unsaved changes).
    if !changed.is_empty() {
        let d = s.doc_mut();
        for (b, now) in next {
            if now.is_empty() {
                d.cut_list.remove(&b);
            } else {
                d.cut_list.insert(b, now);
            }
        }
    }
    Ok(json!({"bodies": bodies, "changed": changed}))
}

/// Most stocks a design gives its own lengths.
const MAX_DESIGN_STOCKS: usize = 2000;

fn set_stock_lengths(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.stock_lengths";
    let profile = str_(p, "profile").map(str::trim).unwrap_or_default().to_string();
    if profile.is_empty() || profile.chars().count() > solidcraft_doc::weld::MAX_STOCK_NAME || profile.chars().any(char::is_control) {
        return Err(bad(cmd, "`profile` must name a stock"));
    }
    if p.get("lengths").is_none() {
        return Err(bad(cmd, "`lengths` is required (empty: the library profile's again)"));
    }
    let lengths = stock_lengths(s, p, "lengths", cmd)?;
    if !lengths.is_empty() && !s.doc.stock_lengths.contains_key(&profile) && s.doc.stock_lengths.len() >= MAX_DESIGN_STOCKS {
        return Err(bad(cmd, format!("a design gives at most {MAX_DESIGN_STOCKS} stocks lengths of its own")));
    }
    // Only a real change touches the design (an undo step, unsaved changes).
    if s.doc.stock_lengths.get(&profile).map_or(&[][..], Vec::as_slice) != lengths.as_slice() {
        let d = s.doc_mut();
        if lengths.is_empty() {
            d.stock_lengths.remove(&profile);
        } else {
            d.stock_lengths.insert(profile.clone(), lengths.clone());
        }
    }
    Ok(json!({"profile": profile, "lengths_mm": lengths}))
}

/// A sub-weldment's name from `p.name`: trimmed, 1…128 characters, used by no other one.
fn sub_weldment_name(s: &Session, p: &Value, except: Option<u64>, cmd: &str) -> Result<Option<String>> {
    let Some(v) = p.get("name") else { return Ok(None) };
    let name = v.as_str().map(str::trim).filter(|n| !n.is_empty() && n.len() <= 128).ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    if s.doc.sub_weldments.iter().any(|w| w.name == name && Some(w.id) != except) {
        return Err(bad(cmd, format!("there is already a sub-weldment `{name}`")));
    }
    Ok(Some(name.to_string()))
}

fn sub_weldment(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.sub_weldment";
    let bodies = string_list(p, "bodies");
    if bodies.is_empty() {
        return Err(bad(cmd, "`bodies` must list body names"));
    }
    if bodies.len() > 10_000 {
        return Err(bad(cmd, "`bodies` lists too many bodies"));
    }
    for b in &bodies {
        if !listable(s, b) {
            return Err(bad(cmd, format!("no body `{b}`")));
        }
    }
    let into = if p.get("sub_weldment").is_some() { Some(sub_weldment_index(s, p, "sub_weldment", cmd)?) } else { None };
    let name = sub_weldment_name(s, p, into.and_then(|i| s.doc.sub_weldments.get(i)).map(|w| w.id), cmd)?;
    let id = match into.and_then(|i| s.doc.sub_weldments.get(i)) {
        Some(w) => w.id,
        None => s.doc_mut().allocate_id()?,
    };
    let name = match (name, into.and_then(|i| s.doc.sub_weldments.get(i))) {
        (Some(n), _) => n,
        (None, Some(w)) => w.name.clone(),
        (None, None) => (1..).map(|n| format!("Sub-weldment {n}")).find(|n| !s.doc.sub_weldments.iter().any(|w| w.name == *n)).unwrap_or_default(),
    };
    let d = s.doc_mut();
    for w in &mut d.sub_weldments {
        w.bodies.retain(|b| !bodies.contains(b));
    }
    match d.sub_weldments.iter_mut().find(|w| w.id == id) {
        Some(w) => {
            w.name = name.clone();
            w.bodies.extend(bodies.iter().cloned());
        }
        None => d.sub_weldments.push(solidcraft_doc::SubWeldment { id, name: name.clone(), bodies: bodies.clone() }),
    }
    d.sub_weldments.retain(|w| !w.bodies.is_empty());
    let all = d.sub_weldments.iter().find(|w| w.id == id).map(|w| w.bodies.clone()).unwrap_or_default();
    Ok(json!({"sub_weldment": id, "name": name, "bodies": all}))
}

fn rename_sub_weldment(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.rename_sub_weldment";
    let i = sub_weldment_index(s, p, "sub_weldment", cmd)?;
    let id = s.doc.sub_weldments.get(i).map(|w| w.id).unwrap_or_default();
    let name = sub_weldment_name(s, p, Some(id), cmd)?.ok_or_else(|| bad(cmd, "`name` must be 1…128 characters"))?;
    if let Some(w) = s.doc_mut().sub_weldments.get_mut(i) {
        w.name = name.clone();
    }
    Ok(json!({"sub_weldment": id, "name": name}))
}

fn dissolve_sub_weldment(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.dissolve_sub_weldment";
    let i = sub_weldment_index(s, p, "sub_weldment", cmd)?;
    let Some(w) = s.doc.sub_weldments.get(i) else { return Err(bad(cmd, "no such sub-weldment")) };
    let id = w.id;
    let only = string_list(p, "bodies");
    if let Some(b) = only.iter().find(|b| !w.bodies.contains(b)) {
        return Err(bad(cmd, format!("`{b}` is not in sub-weldment `{}`", w.name)));
    }
    let leaving: Vec<String> = if only.is_empty() { w.bodies.clone() } else { only };
    let d = s.doc_mut();
    if let Some(w) = d.sub_weldments.get_mut(i) {
        w.bodies.retain(|b| !leaving.contains(b));
    }
    d.sub_weldments.retain(|w| !w.bodies.is_empty());
    Ok(json!({"sub_weldment": id, "bodies": leaving}))
}

fn cut_list(s: &mut Session, p: &Value) -> Result<Value> {
    cut_list_json(s, p, "weld.cut_list")
}

fn export_cut_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.export_cut_list";
    let format = str_(p, "format").unwrap_or("csv").to_ascii_lowercase();
    if !matches!(format.as_str(), "csv" | "json") {
        return Err(bad(cmd, "`format` must be csv or json"));
    }
    let list = cut_list_json(s, p, cmd)?;
    let rows = list["rows"].as_array().cloned().unwrap_or_default();
    let text = if format == "json" {
        let mut v = list.clone();
        v["schema"] = json!("solidcraft-cut-list/1");
        serde_json::to_string_pretty(&v).map_err(|e| EngineError::Other(e.to_string()))?
    } else {
        // Text fields are always quoted: profile, material and body names may hold commas or quotes.
        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let end = |e: &Value| {
            let mitre = e["angle_deg"].as_f64().or_else(|| e["mitre_deg"].as_f64()).unwrap_or(0.0);
            format!("{},{:.4},{:.4}", e["cut"].as_str().unwrap_or("square"), mitre, e["bevel_deg"].as_f64().unwrap_or(0.0))
        };
        // A column per free property any row has, after the fixed ones.
        let property_names = |rows: &[Value]| -> Vec<String> {
            let mut names: Vec<String> = rows.iter().filter_map(|r| r["properties"].as_object()).flat_map(|m| m.keys().cloned()).collect();
            names.sort();
            names.dedup();
            names
        };
        // With sub-weldments, each row's (empty for the weldment's own) before the properties.
        let sections = list["sub_weldments"].as_array().is_some_and(|v| !v.is_empty());
        let property_cells = |r: &Value, names: &[String]| -> String {
            let section = if sections { format!(",{}", quote(r["sub_weldment"].as_str().unwrap_or_default())) } else { String::new() };
            section + &names.iter().map(|n| format!(",{}", quote(r["properties"][n].as_str().unwrap_or_default()))).collect::<String>()
        };
        let property_heads = |names: &[String]| -> String {
            let section = if sections { ",sub_weldment" } else { "" };
            section.to_string() + &names.iter().map(|n| format!(",{}", quote(n))).collect::<String>()
        };
        let member_props = property_names(&rows);
        // With bent members, each row's cut length and bends (feed/rotation/angle/radius, `;`
        // between bends, `formed` for a member formed to a free-form shape) and tail after the
        // profile.
        let bent = rows.iter().any(|r| r["bends"].as_array().is_some_and(|b| !b.is_empty()) || r["formed"].as_bool() == Some(true));
        let bent_cells = |r: &Value| -> String {
            if !bent {
                return String::new();
            }
            let steps = if r["formed"].as_bool() == Some(true) {
                "formed".to_string()
            } else {
                let steps = r["bends"].as_array().into_iter().flatten().map(|b| {
                    let n = |k: &str| b[k].as_f64().unwrap_or(0.0);
                    format!("{:.3}/{:.2}/{:.2}/{:.3}", n("feed_mm"), n("rotation_deg"), n("angle_deg"), n("radius_mm"))
                });
                steps.collect::<Vec<_>>().join(";")
            };
            let tail = r["tail_mm"].as_f64().map(|t| format!("{t:.3}")).unwrap_or_default();
            format!(",{:.3},{},{tail}", r["cut_length_mm"].as_f64().unwrap_or(0.0), quote(&steps))
        };
        let mut csv = format!(
            "item,qty,description,length_mm,end1,end1_mitre_deg,end1_bevel_deg,end2,end2_mitre_deg,end2_bevel_deg,mass_kg,material,members,mirror_of,mirrored,profile{}{}\r\n",
            if bent { ",cut_length_mm,bends_feed_rotation_angle_radius,tail_mm" } else { "" },
            property_heads(&member_props)
        );
        for r in &rows {
            let names = |key: &str| r[key].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(";");
            csv.push_str(&format!(
                "{},{},{},{:.3},{},{},{:.4},{},{},{},{},{}{}{}\r\n",
                r["item"],
                r["qty"],
                quote(r["description"].as_str().unwrap_or_default()),
                r["length_mm"].as_f64().unwrap_or(0.0),
                end(&r["end1"]),
                end(&r["end2"]),
                r["mass_kg"].as_f64().unwrap_or(0.0),
                quote(r["material"].as_str().unwrap_or_default()),
                quote(&names("members")),
                r["mirror_of"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
                quote(&names("mirrored")),
                quote(r["profile"].as_str().unwrap_or_default()),
                bent_cells(r),
                property_cells(r, &member_props)
            ));
        }
        // The plates follow as a second table, after a blank line.
        let plates = list["plates"].as_array().cloned().unwrap_or_default();
        let plate_props = property_names(&plates);
        if !plates.is_empty() {
            csv.push_str(&format!(
                "\r\nitem,qty,plate,thickness_mm,width_mm,height_mm,area_mm2,mass_kg,material,bodies,description{}\r\n",
                property_heads(&plate_props)
            ));
        }
        for r in &plates {
            let names = r["bodies"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(";");
            let mm = |key: &str| r[key].as_f64().unwrap_or(0.0);
            csv.push_str(&format!(
                "{},{},{},{:.3},{:.3},{:.3},{:.2},{:.4},{},{},{}{}\r\n",
                r["item"],
                r["qty"],
                r["kind"].as_str().unwrap_or_default(),
                mm("thickness_mm"),
                mm("width_mm"),
                mm("height_mm"),
                mm("area_mm2"),
                mm("mass_kg"),
                quote(r["material"].as_str().unwrap_or_default()),
                quote(&names),
                quote(r["description"].as_str().unwrap_or_default()),
                property_cells(r, &plate_props)
            ));
        }
        // Then the other bodies, a third table after another blank line.
        let others = list["bodies"].as_array().cloned().unwrap_or_default();
        let other_props = property_names(&others);
        if !others.is_empty() {
            csv.push_str(&format!(
                "\r\nitem,qty,body,length_mm,width_mm,thickness_mm,volume_mm3,mass_kg,material,bodies,description{}\r\n",
                property_heads(&other_props)
            ));
        }
        for r in &others {
            let names = r["bodies"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(";");
            let mm = |key: &str| r[key].as_f64().unwrap_or(0.0);
            csv.push_str(&format!(
                "{},{},{},{:.3},{:.3},{:.3},{:.2},{:.4},{},{},{}{}\r\n",
                r["item"],
                r["qty"],
                r["kind"].as_str().unwrap_or_default(),
                mm("length_mm"),
                mm("width_mm"),
                mm("thickness_mm"),
                mm("volume_mm3"),
                mm("mass_kg"),
                quote(r["material"].as_str().unwrap_or_default()),
                quote(&names),
                quote(r["description"].as_str().unwrap_or_default()),
                property_cells(r, &other_props)
            ));
        }
        // Then, where any stock's lengths are known, the bars to buy and cut: a fourth table.
        let stock = list["stock"].as_array().cloned().unwrap_or_default();
        if stock.iter().any(|x| x["bars"].as_array().is_some_and(|b| !b.is_empty()) || x["too_long"].as_array().is_some_and(|b| !b.is_empty())) {
            csv.push_str("\r\nstock,bar,length_mm,items,cuts_mm,drop_mm\r\n");
            for x in &stock {
                let name = quote(x["description"].as_str().unwrap_or_default());
                for (i, b) in x["bars"].as_array().into_iter().flatten().enumerate() {
                    let list = |key: &str, f: &dyn Fn(&Value) -> String| b[key].as_array().into_iter().flatten().map(f).collect::<Vec<_>>().join(";");
                    csv.push_str(&format!(
                        "{name},{},{:.3},{},{},{:.3}\r\n",
                        i + 1,
                        b["length_mm"].as_f64().unwrap_or(0.0),
                        quote(&list("items", &|v| v.to_string())),
                        quote(&list("cuts_mm", &|v| format!("{:.3}", v.as_f64().unwrap_or(0.0)))),
                        b["drop_mm"].as_f64().unwrap_or(0.0)
                    ));
                }
                for m in x["too_long"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    csv.push_str(&format!("{name},too long,,{},,\r\n", quote(m)));
                }
            }
        }
        // Then the weld table, where there are weld beads.
        let welds = list["welds"].as_array().cloned().unwrap_or_default();
        if !welds.is_empty() {
            csv.push_str(&format!(
                "\r\nweld,qty,kind,size_mm,length_mm,welds,intermittent_length_mm,pitch_mm,field,volume_mm3,mass_kg,beads,bodies,note{}\r\n",
                if sections { ",sub_weldment" } else { "" }
            ));
        }
        for r in &welds {
            let mm = |v: &Value| v.as_f64().map(|x| format!("{x:.3}")).unwrap_or_default();
            let beads = r["beads"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(";");
            let bodies = r["bodies"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|pair| pair.as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("+"))
                .collect::<Vec<_>>()
                .join(";");
            let section = if sections { format!(",{}", quote(r["sub_weldment"].as_str().unwrap_or_default())) } else { String::new() };
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{}{section}\r\n",
                r["weld"],
                r["qty"],
                r["kind"].as_str().unwrap_or_default(),
                mm(&r["size_mm"]),
                mm(&r["length_mm"]),
                r["welds"],
                mm(&r["intermittent"]["length_mm"]),
                mm(&r["intermittent"]["pitch_mm"]),
                r["field"].as_bool().unwrap_or(false),
                r["volume_mm3"].as_f64().map(|x| format!("{x:.2}")).unwrap_or_default(),
                r["mass_kg"].as_f64().map(|x| format!("{x:.4}")).unwrap_or_default(),
                quote(&beads),
                quote(&bodies),
                quote(r["note"].as_str().unwrap_or_default())
            ));
        }
        csv
    };
    if let Some(path) = str_(p, "path").filter(|x| !x.trim().is_empty()) {
        solidcraft_io::vfs::write(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        return Ok(json!({"path": path, "format": format, "rows": rows.len()}));
    }
    Ok(json!({"format": format, "text": text, "rows": rows.len()}))
}
