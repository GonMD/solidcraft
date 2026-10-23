//! Weldment Trim/Extend through the commands: members of a square tube 50 × 50 × 3 (area
//! 564 mm²) cut back or lengthened to planes, faces and other members, checked by volume, extent
//! and cut list.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::Session;

const AREA: f64 = 564.0;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// The tube profile `Tube` and a sketch `Lines` on XY: `rail` from the origin to (950, 0) and
/// `post` across it at x = 600, from (600, −300) to (600, 300); `Rail` and `Post` are members of
/// two Structural Members, the post's made after the rail's (so the rail runs through it).
fn setup() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Tube"}));
    run(&mut s, "sketch.line", json!({"points": [[-25, -25], [25, -25], [25, 25], [-25, 25]], "closed": true}));
    run(&mut s, "sketch.line", json!({"points": [[-22, -22], [22, -22], [22, 22], [-22, 22]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Lines"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [950, 0]], "ids": ["rail"]}));
    run(&mut s, "sketch.line", json!({"points": [[600, -300], [600, 300]], "ids": ["post"]}));
    run(&mut s, "sketch.finish", json!({}));
    for (line, name) in [("rail", "Rail"), ("post", "Post")] {
        let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Lines", "path": [line], "body_name": name, "join_earlier": false});
        run(&mut s, "weld.member", p);
    }
    s
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.world_state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap()
}

fn near(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-6, "{what}: {got} ≠ {want}");
}

fn trim(s: &mut Session, p: Value) -> Result<Value, String> {
    s.execute("weld.trim_extend", &p).map_err(|e| e.to_string())
}

/// The cut-list row of `body`.
fn row_of(s: &mut Session, body: &str) -> Value {
    let list = run(s, "weld.cut_list", json!({}));
    list["rows"].as_array().unwrap().iter().find(|r| r["members"].as_array().unwrap().iter().any(|m| m == body)).cloned().unwrap()
}

/// A plane past the rail's end lengthens it; one short of its end cuts it back; one past its
/// start lengthens it the other way. The cut list follows.
#[test]
fn a_member_extends_and_trims_to_a_plane() {
    let mut s = setup();
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [1200, 0, 0], "normal": [1, 0, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 1200.0, "lengthened to 1200");
    near(measure(&s, "Rail").bbox.max.x, 1200.0, "end");
    near(row_of(&mut s, "Rail")["length_mm"].as_f64().unwrap(), 1200.0, "cut list");
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [800, 0, 0], "normal": [1, 0, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 800.0, "cut back to 800");
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [-100, 0, 0], "normal": [-1, 0, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 900.0, "start lengthened to −100");
    near(measure(&s, "Rail").bbox.min.x, -100.0, "start");
    assert_eq!(measure(&s, "Rail").faces, 10, "made again whole: no seams");
}

/// A plane at 45° cuts a mitre: the volume is the profile times the line to where the plane
/// crosses it, and the cut list reads 45°.
#[test]
fn a_leaning_plane_cuts_a_mitre() {
    let mut s = setup();
    let n = 0.5f64.sqrt();
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [1000, 0, 0], "normal": [n, n, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 1000.0, "volume");
    let row = row_of(&mut s, "Rail");
    assert_eq!(row["end2"]["cut"], "mitre", "{row}");
    near(row["end2"]["angle_deg"].as_f64().unwrap(), 45.0, "mitre angle");
    near(row["length_mm"].as_f64().unwrap(), 1025.0, "long point");
}

/// An earlier member cut back to a later one: the rail ends against the post's near face, a
/// gap short of it; editing the gap moves it.
#[test]
fn an_earlier_member_is_cut_back_to_a_later_one() {
    let mut s = setup();
    let r = trim(&mut s, json!({"members": ["Rail"], "to": ["Post"], "name": "Rail to post"})).unwrap();
    assert_eq!(r["name"], "Rail to post");
    near(measure(&s, "Rail").volume, AREA * 575.0, "rail to the post's face");
    near(measure(&s, "Rail").bbox.max.x, 575.0, "end");
    near(measure(&s, "Post").volume, AREA * 600.0, "the post is left as it is");
    run(&mut s, "timeline.edit", json!({"feature": "Rail to post", "set": {"gap": "5 mm"}}));
    near(measure(&s, "Rail").volume, AREA * 570.0, "a 5 mm gap");
    run(&mut s, "edit.undo", json!({}));
    near(measure(&s, "Rail").volume, AREA * 575.0, "undone");
    run(&mut s, "edit.redo", json!({}));
    near(measure(&s, "Rail").volume, AREA * 570.0, "redone");
}

/// A planar face of another body as the boundary.
#[test]
fn a_member_extends_to_a_face() {
    let mut s = setup();
    run(&mut s, "solid.box", json!({"length": 100, "width": 400, "height": 100, "corner": [1100, -200, -50], "body_name": "Plate"}));
    trim(&mut s, json!({"members": ["Rail"], "face": {"body": "Plate", "point": [1100, 0, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 1100.0, "up to the plate's face");
}

/// `mode` keeps a member that would be moved the other way as it is, with a warning.
#[test]
fn the_mode_limits_which_way_a_member_moves() {
    let mut s = setup();
    let r = trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [1200, 0, 0], "normal": [1, 0, 0]}, "mode": "trim"})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 950.0, "left as it is");
    assert!(r["warning"].as_str().is_some_and(|w| w.contains("lengthen")), "{r}");
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [800, 0, 0], "normal": [1, 0, 0]}, "mode": "extend"})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 950.0, "not cut back");
}

/// A member a later feature changed can still be cut back (on the plane), not lengthened.
#[test]
fn a_changed_member_is_only_cut_back() {
    let mut s = setup();
    // A slot through the top wall only (z 22…25), so the rail stays one piece.
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 10, "corner": [100, -10, 20], "body_name": "Notch"}));
    run(&mut s, "solid.combine", json!({"target": "Rail", "tools": ["Notch"], "operation": "cut"}));
    let notched = measure(&s, "Rail").volume;
    near(notched, AREA * 950.0 - 600.0, "notched");
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [800, 0, 0], "normal": [1, 0, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, notched - AREA * 150.0, "cut back");
    let e = trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [1200, 0, 0], "normal": [1, 0, 0]}})).unwrap_err();
    assert!(e.contains("not supported yet") && e.contains("Rail"), "{e}");
    // Once cut as it was, it is still the changed member: cutting it back again keeps the slot.
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [700, 0, 0], "normal": [1, 0, 0]}})).unwrap();
    near(measure(&s, "Rail").volume, notched - AREA * 250.0, "cut back again, slot kept");
}

#[test]
fn wrong_trims_are_refused() {
    let mut s = setup();
    let x = json!({"origin": [800, 0, 0], "normal": [1, 0, 0]});
    for (p, want) in [
        (json!({"members": ["Rail"]}), "one boundary"),
        (json!({"members": ["Rail"], "plane": x, "to": ["Post"]}), "one boundary"),
        (json!({"members": [], "plane": x}), "members"),
        (json!({"members": ["Nope"], "plane": x}), "not a structural member"),
        (json!({"members": ["Rail"], "to": ["Rail"]}), "itself"),
        (json!({"members": ["Rail"], "plane": x, "mode": "sideways"}), "mode"),
        (json!({"members": ["Rail"], "plane": {"origin": [0, 100, 0], "normal": [0, 1, 0]}}), "along"),
        (json!({"members": ["Rail"], "plane": x, "gap": "-1 mm"}), "gap"),
    ] {
        let e = trim(&mut s, p.clone()).unwrap_err();
        assert!(e.contains(want), "{p}: {e}");
    }
    near(measure(&s, "Rail").volume, AREA * 950.0, "nothing changed");
}

/// A later Structural Member sees a member as trimmed: past the rail's old end, a brace
/// butts against the lengthened rail (and wouldn't without the extension); saved and reopened,
/// the frame is the same.
#[test]
fn later_members_join_the_trimmed_member() {
    let mut s = setup();
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [1200, 0, 0], "normal": [1, 0, 0]}})).unwrap();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Brace"}));
    run(&mut s, "sketch.line", json!({"points": [[1100, 300], [1100, 0]], "ids": ["brace"]}));
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Brace", "path": ["brace"], "body_name": "Brace"});
    run(&mut s, "weld.member", p);
    near(measure(&s, "Brace").volume, AREA * 275.0, "butted against the rail's side");
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    for b in ["Rail", "Post", "Brace"] {
        near(measure(&reopened, b).volume, measure(&s, b).volume, b);
    }
}

/// A round rail (tube Ø50 × 3) along X and a smaller round post (Ø30 × 3) above it along Y,
/// from y = 300 down to y = 100.
fn round_setup() -> Session {
    let mut s = Session::default();
    for (name, plane, r) in [("Round", "YZ", 25.0), ("Small", "XZ", 15.0)] {
        run(&mut s, "sketch.create", json!({"plane": plane, "name": name}));
        run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": r}));
        run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": r - 3.0}));
        run(&mut s, "sketch.finish", json!({}));
    }
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Lines"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [950, 0]], "ids": ["rail"]}));
    run(&mut s, "sketch.line", json!({"points": [[600, 300], [600, 100]], "ids": ["post"]}));
    run(&mut s, "sketch.finish", json!({}));
    for (line, name, profile, r) in [("rail", "Rail", "Round", 23.5), ("post", "Post", "Small", 13.5)] {
        let p = json!({"sketch": profile, "profiles": [{"point": [r, 0]}], "path_sketch": "Lines", "path": [line], "body_name": name});
        run(&mut s, "weld.member", p);
    }
    s
}

/// The post (rings of radius 12…15 about its line) from y = 300 down to the rail's outside
/// (radius 25 about the X axis) moved `gap` up: ∫ (300 − gap − √(25² − z²)) dA.
fn coped_post_volume(gap: f64) -> f64 {
    let (n, m) = (200, 720);
    let mut v = 0.0;
    for i in 0..n {
        let r = 12.0 + 3.0 * (i as f64 + 0.5) / n as f64;
        for j in 0..m {
            let z = r * (std::f64::consts::TAU * (j as f64 + 0.5) / m as f64).sin();
            v += (300.0 - gap - (625.0 - z * z).sqrt()) * r * (3.0 / n as f64) * (std::f64::consts::TAU / m as f64);
        }
    }
    v
}

/// A round post coped onto a round rail: lengthened to the rail and shaped to its outside. The
/// cut list reads `coped`, with the long point where the post's sides reach down past the rail's
/// top (√(25² − 15²) = 20 from its line).
#[test]
fn a_round_post_is_coped_to_a_round_rail() {
    let mut s = round_setup();
    trim(&mut s, json!({"members": ["Post"], "to": ["Rail"], "cope": true, "name": "Cope"})).unwrap();
    let v = measure(&s, "Post").volume;
    assert!((v - coped_post_volume(0.0)).abs() < coped_post_volume(0.0) * 2e-3, "volume {v} ≠ {}", coped_post_volume(0.0));
    near(measure(&s, "Post").bbox.min.y, 20.0, "the post's sides reach down to the rail's sides");
    let row = row_of(&mut s, "Post");
    assert_eq!(row["end2"]["cut"], "coped", "{row}");
    let e = s.execute("weld.end_cap", &json!({"ends": [{"member": "Post", "end": "end"}], "thickness": 3})).unwrap_err().to_string();
    assert!(e.contains("no open end to cap"), "{e}");
    // Measured on the outlines as polylines (within 0.01 mm of the circles).
    let long = row["length_mm"].as_f64().unwrap();
    assert!((long - 280.0).abs() < 0.02, "long point {long}");
    // A gap moves the cope away from the rail.
    run(&mut s, "timeline.edit", json!({"feature": "Cope", "set": {"gap": "2 mm"}}));
    let v = measure(&s, "Post").volume;
    assert!((v - coped_post_volume(2.0)).abs() < coped_post_volume(2.0) * 2e-3, "volume with a gap {v}");
    // Saved and reopened, the same.
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    near(measure(&reopened, "Post").volume, v, "reopened");
}

/// The volume of member `body` from how it was made (`MemberCut::volume`), `None` where the cut
/// list measures it instead.
fn made_volume(s: &Session, body: &str) -> Option<f64> {
    let st = s.world_state();
    let w = st.welds.iter().find(|w| w.body == body).unwrap_or_else(|| panic!("no member {body}"));
    w.volume(&st.body(body).unwrap().body)
}

/// The cut list takes a member's volume from how it was made (profile area times the length
/// between its cut planes), with no tessellation, while its body is still that shape: square and
/// mitred ends alike, and after a recolour; it matches the measured body. A coped member is
/// measured.
#[test]
fn member_volumes_come_from_how_they_were_made() {
    let mut s = setup();
    let n = 0.5f64.sqrt();
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [1000, 0, 0], "normal": [n, n, 0]}})).unwrap();
    for body in ["Rail", "Post"] {
        let v = made_volume(&s, body).unwrap_or_else(|| panic!("{body}: not from how it was made"));
        near(v, measure(&s, body).volume, body);
    }
    near(made_volume(&s, "Rail").unwrap(), AREA * 1000.0, "mitred rail");
    run(&mut s, "appearance.assign", json!({"bodies": ["Rail"], "color": "#abcdef"}));
    near(made_volume(&s, "Rail").unwrap_or_default(), AREA * 1000.0, "recoloured");
    let mut s = round_setup();
    assert!(made_volume(&s, "Post").is_some(), "a square-cut round post");
    trim(&mut s, json!({"members": ["Post"], "to": ["Rail"], "cope": true})).unwrap();
    assert_eq!(made_volume(&s, "Post"), None, "a coped post is measured");
}

/// A coped post and a flat-cut one of the same length are different parts; trimming the coped
/// one's other end keeps the cope.
#[test]
fn a_cope_stays_with_its_end() {
    let mut s = round_setup();
    trim(&mut s, json!({"members": ["Post"], "to": ["Rail"], "cope": true})).unwrap();
    let coped = measure(&s, "Post").volume;
    trim(&mut s, json!({"members": ["Post"], "plane": {"origin": [0, 250, 0], "normal": [0, 1, 0]}})).unwrap();
    // Volumes of curved booleans are measured to about 1e-5.
    let want = coped - 50.0 * std::f64::consts::PI * (225.0 - 144.0);
    let v = measure(&s, "Post").volume;
    assert!((v - want).abs() < want * 1e-4, "cut back 50 at the top, cope kept: {v} ≠ {want}");
    let row = row_of(&mut s, "Post");
    assert_eq!((row["end1"]["cut"].as_str(), row["end2"]["cut"].as_str()), (Some("square"), Some("coped")), "{row}");
}

/// A round post coped onto the round face of a body that isn't a member (a solid bar Ø50 along
/// the rail's line): shaped to its cylinder as to the rail. The face's own hole wall, and a flat
/// cut to a curved face, are refused.
#[test]
fn a_round_post_is_coped_to_a_round_face() {
    let mut s = round_setup();
    run(&mut s, "solid.cylinder", json!({"radius": 25, "height": 950, "base": [0, 0, 0], "axis": [1, 0, 0], "operation": "new", "body_name": "Bar"}));
    let face = json!({"body": "Bar", "point": [600, 0, 25]});
    trim(&mut s, json!({"members": ["Post"], "face": face, "cope": true, "name": "Cope"})).unwrap();
    let v = measure(&s, "Post").volume;
    assert!((v - coped_post_volume(0.0)).abs() < coped_post_volume(0.0) * 2e-3, "volume {v} ≠ {}", coped_post_volume(0.0));
    near(measure(&s, "Post").bbox.min.y, 20.0, "the post's sides reach down beside the bar");
    let row = row_of(&mut s, "Post");
    assert_eq!(row["end2"]["cut"], "coped", "{row}");
    assert!((row["length_mm"].as_f64().unwrap() - 280.0).abs() < 0.02, "{row}");
    // Reopened, the face is found again.
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    near(measure(&reopened, "Post").volume, v, "reopened");
    for (p, want) in [
        (json!({"members": ["Post"], "face": {"body": "Rail", "point": [300, 0, 22]}, "cope": true}), "inside of a hole"),
        (json!({"members": ["Post"], "face": face, "cope": false}), "curved face"),
        (json!({"members": ["Post"], "face": {"body": "Post", "point": [600, 250, 15]}, "cope": true}), "own face"),
    ] {
        let e = trim(&mut s, p.clone()).unwrap_err();
        assert!(e.contains(want), "{p}: {e}");
    }
}

#[test]
fn a_cope_needs_members() {
    let mut s = round_setup();
    let e = trim(&mut s, json!({"members": ["Post"], "plane": {"origin": [0, 50, 0], "normal": [0, 1, 0]}, "cope": true})).unwrap_err();
    assert!(e.contains("cope"), "{e}");
}

/// A square post coped onto a square rail of the same tube, their sides in the same planes
/// (z = ±25): the cope takes the rail's outside away, which leaves the post cut flat on the
/// rail's top face (y = 25). Turned 45° about its line, the rail's corner cuts a V into the post.
#[test]
fn a_square_post_is_coped_to_a_rail_of_the_same_tube() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Up"}));
    run(&mut s, "sketch.line", json!({"points": [[300, 300], [300, 100]], "ids": ["up"]}));
    run(&mut s, "sketch.finish", json!({}));
    let p =
        json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Up", "path": ["up"], "body_name": "Up", "join_earlier": false});
    run(&mut s, "weld.member", p);
    trim(&mut s, json!({"members": ["Up"], "to": ["Rail"], "cope": true, "name": "Cope"})).unwrap();
    let m = measure(&s, "Up");
    near(m.volume, AREA * 275.0, "flat on the rail's top");
    near(m.bbox.min.y, 25.0, "down to the rail's top");
    near(m.bbox.max.z - m.bbox.min.z, 50.0, "sides kept");
    let row = row_of(&mut s, "Up");
    assert_eq!(row["end2"]["cut"], "coped", "{row}");
    assert!((row["length_mm"].as_f64().unwrap() - 275.0).abs() < 0.02, "{row}");
    // A diamond rail (the tube turned 45°) along y = −600 and a post down to it from y = −300:
    // the rail stands c = 25√2 above its line at z = 0, c − |z| beside it, so the post loses
    // ∫ (c − |z|) dA over its walls = 564 c − 9954 below its cut through the rail's line.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Low"}));
    run(&mut s, "sketch.line", json!({"points": [[0, -600], [950, -600]], "ids": ["low"]}));
    run(&mut s, "sketch.line", json!({"points": [[300, -300], [300, -500]], "ids": ["down"]}));
    run(&mut s, "sketch.finish", json!({}));
    for (line, name, angle) in [("low", "Diamond", "45 deg"), ("down", "Down", "0 deg")] {
        let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Low", "path": [line], "body_name": name, "angle": angle, "join_earlier": false});
        run(&mut s, "weld.member", p);
    }
    trim(&mut s, json!({"members": ["Down"], "to": ["Diamond"], "cope": true})).unwrap();
    let c = 25.0 * 2f64.sqrt();
    let m = measure(&s, "Down");
    near(m.volume, AREA * 300.0 - (564.0 * c - 9954.0), "a V cut by the diamond's corner");
    near(m.bbox.min.y, -600.0 + c - 25.0, "the post's corners reach down beside the diamond");
}
