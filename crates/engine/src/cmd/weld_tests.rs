//! Weldment Structural Members through the commands, checked against analytic values: a square
//! tube 50 × 50 × 3 (sharp corners, area 50² − 44² = 564 mm²) along straight skeleton lines.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::Session;

const AREA: f64 = 564.0;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// A tube profile `Tube` on the YZ plane (two nested squares about the origin) and a skeleton
/// `Frame` on XY: a 950 × 550 rectangle (lines `l1`…`l4`, from the origin anticlockwise).
fn setup() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Tube"}));
    run(&mut s, "sketch.line", json!({"points": [[-25, -25], [25, -25], [25, 25], [-25, 25]], "closed": true}));
    run(&mut s, "sketch.line", json!({"points": [[-22, -22], [22, -22], [22, 22], [-22, 22]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Frame"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [950, 0], [950, 550], [0, 550]], "closed": true, "ids": ["l1", "l2", "l3", "l4"]}));
    run(&mut s, "sketch.finish", json!({}));
    s
}

fn member(s: &mut Session, path: &[&str], extra: Value) -> Result<Value, String> {
    let mut p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Frame", "path": path, "body_name": "Rail"});
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    s.execute("weld.member", &p).map_err(|e| e.to_string())
}

fn trim(s: &mut Session, p: Value) -> Result<Value, String> {
    s.execute("weld.trim_extend", &p).map_err(|e| e.to_string())
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.world_state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap()
}

fn near(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-6, "{what}: {got} ≠ {want}");
}

/// One free member is the profile times the line's length, cut square at both ends: 4 outer,
/// 4 inner and 2 end faces.
#[test]
fn a_straight_member_is_the_profile_times_its_length() {
    let mut s = setup();
    member(&mut s, &["l1"], json!({})).unwrap();
    let m = measure(&s, "Rail");
    near(m.volume, AREA * 950.0, "volume");
    assert_eq!(m.faces, 10, "faces");
    near(m.bbox.min.x, 0.0, "start");
    near(m.bbox.max.x, 950.0, "end");
    near(m.bbox.max.y - m.bbox.min.y, 50.0, "width");
    near(m.bbox.max.z - m.bbox.min.z, 50.0, "height");
}

/// The mitred rectangle: four members, each the profile times its centre line, together the
/// outer 1000 × 600 frame.
#[test]
fn a_mitred_rectangle_has_one_body_per_line() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    let names = ["Rail", "Rail (2)", "Rail (3)", "Rail (4)"];
    let mut total = 0.0;
    for (name, length) in names.iter().zip([950.0, 550.0, 950.0, 550.0]) {
        let m = measure(&s, name);
        near(m.volume, AREA * length, name);
        assert_eq!(m.faces, 10, "{name} faces");
        total += m.volume;
    }
    near(total, 1_692_000.0, "total");
    let long = measure(&s, "Rail");
    near(long.bbox.min.x, -25.0, "outside start");
    near(long.bbox.max.x, 975.0, "outside end");
    let short = measure(&s, "Rail (2)");
    near(short.bbox.max.y - short.bbox.min.y, 600.0, "short member outside length");
}

/// The profile turns about each line; a turned square of the same size has the same volume.
#[test]
fn the_profile_turns_by_its_angle() {
    let mut s = setup();
    member(&mut s, &["l1"], json!({"angle": "45 deg"})).unwrap();
    let m = measure(&s, "Rail");
    near(m.volume, AREA * 950.0, "volume");
    near(m.bbox.max.y - m.bbox.min.y, 50.0 * 2f64.sqrt(), "turned width");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"angle": "0 deg"}}));
    near(measure(&s, "Rail").bbox.max.y - measure(&s, "Rail").bbox.min.y, 50.0, "width again");
}

/// Members of a 3D sketch: an L of two lines meeting at a right angle, mitred.
#[test]
fn members_follow_3d_sketch_lines() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Leg"}));
    run(&mut s, "sketch.options", json!({"sketch_3d": true}));
    let r = run(&mut s, "sketch.line3d", json!({"points": [[0, 0, 0], [0, 0, 700], [400, 0, 700]]}));
    run(&mut s, "sketch.finish", json!({}));
    let ids: Vec<String> = r["curves"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Leg", "path": ids, "body_name": "Leg"});
    run(&mut s, "weld.member", p);
    near(measure(&s, "Leg").volume, AREA * 700.0, "upright");
    near(measure(&s, "Leg (2)").volume, AREA * 400.0, "arm");
    near(measure(&s, "Leg").bbox.max.z, 725.0, "mitred top");
}

/// Saved and reopened, undone and redone, the frame is the same.
#[test]
fn members_reopen_undo_and_redo() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    let doc = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
    let reopened = Session::new(doc);
    near(measure(&reopened, "Rail (3)").volume, AREA * 950.0, "reopened");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.world_state().body("Rail").is_none(), "undone");
    run(&mut s, "edit.redo", json!({}));
    near(measure(&s, "Rail (4)").volume, AREA * 550.0, "redone");
}

/// Three members at one point: the first two listed make the corner (here they run on in one
/// line, cut square), the third butts against the first's face.
#[test]
fn a_third_member_at_a_joint_butts_against_the_first() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Tee"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [500, 0]], "ids": ["a"]}));
    run(&mut s, "sketch.line", json!({"points": [[500, 0], [1000, 0]], "ids": ["b"]}));
    run(&mut s, "sketch.line", json!({"points": [[500, 0], [500, 400]], "ids": ["c"]}));
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Tee", "path": ["a", "b", "c"], "body_name": "T"});
    let r = run(&mut s, "weld.member", p);
    assert!(!r.to_string().contains("not supported"), "{r}");
    for name in ["T", "T (2)"] {
        near(measure(&s, name).volume, AREA * 500.0, name);
    }
    let stem = measure(&s, "T (3)");
    near(stem.volume, AREA * 375.0, "stem");
    near(stem.bbox.min.y, 25.0, "stem against the rail's face");
}

/// The butted rectangle: listed first, the long rails run through (1000 over their outsides),
/// the stiles butt between them (500); the frame's volume is the mitred one's.
#[test]
fn a_butted_rectangle_runs_the_first_members_through() {
    let mut s = setup();
    member(&mut s, &["l1", "l3", "l2", "l4"], json!({"corner": "butt"})).unwrap();
    let mut total = 0.0;
    for (name, length) in [("Rail", 1000.0), ("Rail (2)", 1000.0), ("Rail (3)", 500.0), ("Rail (4)", 500.0)] {
        let m = measure(&s, name);
        near(m.volume, AREA * length, name);
        assert_eq!(m.faces, 10, "{name} faces");
        total += m.volume;
    }
    near(total, 1_692_000.0, "total");
    let rail = measure(&s, "Rail");
    near(rail.bbox.min.x, -25.0, "rail runs past the stile");
    let stile = measure(&s, "Rail (3)");
    near(stile.bbox.min.y, 25.0, "stile against the rail");
    near(stile.bbox.max.y, 525.0, "stile against the other rail");
}

/// A gap moves each butting end back from the face it butts against.
#[test]
fn a_weld_gap_shortens_the_butting_members() {
    let mut s = setup();
    member(&mut s, &["l1", "l3", "l2", "l4"], json!({"corner": "butt", "gap": "2 mm"})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 1000.0, "rail");
    near(measure(&s, "Rail (3)").volume, AREA * 496.0, "stile");
}

/// One joint joined otherwise: a mitred frame with one butt, its member running through named.
#[test]
fn a_joint_can_be_joined_otherwise_than_the_corner() {
    let mut s = setup();
    let joints = json!({"joints": [{"members": ["l1", "l2"], "corner": "butt", "through": "l2"}]});
    member(&mut s, &["l1", "l2", "l3", "l4"], joints).unwrap();
    // l1: mitred at the origin, butted against l2's face at x = 925.
    let l1 = measure(&s, "Rail");
    near(l1.bbox.max.x, 925.0, "l1 ends at l2's face");
    // The mitre at the origin bisects the profile about the line: as much past 0 as short of it.
    near(l1.volume, AREA * 925.0, "l1");
    // l2 runs through past l1's outside at y = −25.
    let l2 = measure(&s, "Rail (2)");
    near(l2.bbox.min.y, -25.0, "l2 runs through");
    let r = member(&mut s, &["l1", "l2"], json!({"joints": [{"members": ["l1", "l3"], "corner": "butt"}], "body_name": "Other"})).unwrap_err();
    assert!(r.contains("not a member's key"), "{r}");
}

/// `weld.corners` lists each corner once, from the skeleton (before the feature exists) or from
/// the feature: its two members, where they meet and how they are joined. A T (the brace ending
/// on a rail's line) is no corner.
#[test]
fn the_corners_of_a_frame_are_listed() {
    let mut s = setup();
    run(&mut s, "sketch.edit", json!({"sketch": "Frame"}));
    run(&mut s, "sketch.line", json!({"points": [[475, 0], [475, 550]], "ids": ["brace"]}));
    run(&mut s, "sketch.finish", json!({}));
    let path = json!(["l1", "l2", "l3", "l4", "brace"]);
    let joints = json!([{"members": ["l2", "l1"], "corner": "butt", "through": "l2"}]);
    let listed = |v: &Value| -> Vec<(Vec<String>, [f64; 2], String, Option<String>, bool)> {
        v["corners"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    serde_json::from_value(c["members"].clone()).unwrap(),
                    [c["at"][0].as_f64().unwrap(), c["at"][1].as_f64().unwrap()],
                    c["corner"].as_str().unwrap().to_string(),
                    c["through"].as_str().map(str::to_string),
                    c["own"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    let key = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
    let want = vec![
        (key("l1", "l4"), [0.0, 0.0], "mitre".to_string(), None, false),
        (key("l1", "l2"), [950.0, 0.0], "butt".to_string(), Some("l2".to_string()), true),
        (key("l2", "l3"), [950.0, 550.0], "mitre".to_string(), None, false),
        (key("l3", "l4"), [0.0, 550.0], "mitre".to_string(), None, false),
    ];
    let before = run(&mut s, "weld.corners", json!({"path_sketch": "Frame", "path": path, "joints": joints}));
    assert_eq!(listed(&before), want);
    member(&mut s, &["l1", "l2", "l3", "l4", "brace"], json!({"joints": joints})).unwrap();
    let id = s.doc.features.last().unwrap().id;
    assert_eq!(listed(&run(&mut s, "weld.corners", json!({"feature": id}))), want);
    // A butt corner without a joint runs its first member through.
    let butt = run(&mut s, "weld.corners", json!({"path_sketch": "Frame", "path": ["l2", "l1"], "corner": "butt"}));
    assert_eq!(listed(&butt), vec![(key("l2", "l1"), [950.0, 0.0], "butt".to_string(), Some("l2".to_string()), false)]);
    let e = s.execute("weld.corners", &json!({"feature": "Frame"})).unwrap_err().to_string();
    assert!(e.contains("not a Structural Member"), "{e}");
}

/// A member ending on another member's line butts against its face: a brace across the frame.
#[test]
fn a_member_ending_on_another_butts_against_it() {
    let mut s = setup();
    run(&mut s, "sketch.edit", json!({"sketch": "Frame"}));
    run(&mut s, "sketch.line", json!({"points": [[475, 0], [475, 550]], "ids": ["brace"]}));
    run(&mut s, "sketch.finish", json!({}));
    member(&mut s, &["l1", "l2", "l3", "l4", "brace"], json!({})).unwrap();
    let brace = measure(&s, "Rail (5)");
    near(brace.volume, AREA * 500.0, "brace");
    near(brace.bbox.min.y, 25.0, "brace against l1");
    near(brace.bbox.max.y, 525.0, "brace against l3");
    // The rails it ends on are whole.
    near(measure(&s, "Rail").volume, AREA * 950.0, "l1");
}

/// A corner of a 3D frame: two rails mitred, a leg butted up under them.
#[test]
fn a_leg_butts_under_a_mitred_corner() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Corner"}));
    run(&mut s, "sketch.options", json!({"sketch_3d": true}));
    let mut ids = Vec::new();
    for pts in [json!([[0, 0, 0], [600, 0, 0]]), json!([[0, 0, 0], [0, 400, 0]]), json!([[0, 0, 0], [0, 0, -700]])] {
        let r = run(&mut s, "sketch.line3d", json!({ "points": pts }));
        ids.push(r["curves"][0].as_str().unwrap().to_string());
    }
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Corner", "path": ids, "body_name": "C"});
    run(&mut s, "weld.member", p);
    near(measure(&s, "C").volume, AREA * 600.0, "x rail, mitred");
    near(measure(&s, "C (2)").volume, AREA * 400.0, "y rail, mitred");
    let leg = measure(&s, "C (3)");
    near(leg.volume, AREA * 675.0, "leg");
    near(leg.bbox.max.z, -25.0, "leg under the rails");
}

/// Wrong inputs are refused with a message and leave the design unchanged.
#[test]
fn wrong_skeletons_and_profiles_are_refused() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Bent"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [100, 0]], "ids": ["dup"]}));
    run(&mut s, "sketch.line", json!({"points": [[100, 0], [0, 0]], "ids": ["dup2"]}));
    run(&mut s, "sketch.finish", json!({}));
    let features = s.doc.features.len();
    let cases = [
        json!({"path_sketch": "Bent", "path": ["dup", "dup2"]}),
        json!({"end_allowance": "-1 mm"}),
        json!({"path": ["nope"]}),
        json!({"path": []}),
        json!({"profiles": "all"}),
        json!({"angle": "10 mm"}),
        json!({"profiles": [{"point": [100, 100]}]}),
        json!({"corner": "weld"}),
        json!({"gap": "-1 mm"}),
        json!({"joints": [{"members": ["l1"], "corner": "butt"}]}),
        json!({"joints": [{"members": ["l1", "l1"], "corner": "butt"}]}),
        json!({"joints": [{"members": ["l1", "l2"], "corner": "butt", "through": "l3"}]}),
        json!({"joints": "l1"}),
    ];
    for extra in cases {
        let e = member(&mut s, &["l1"], extra.clone());
        assert!(e.is_err(), "{extra} was accepted");
        assert_eq!(s.doc.features.len(), features, "{extra} left a feature");
    }
}

/// A skeleton `Rail` on XY: a line `in` from the origin 300 along X, a quarter arc of radius 100
/// turning toward Y, and a line `out` 400 up from its end: with the arc, 857.08 long.
fn bent_rail(s: &mut Session) -> String {
    run(s, "sketch.create", json!({"plane": "XY", "name": "Rail"}));
    run(s, "sketch.line", json!({"points": [[0, 0], [300, 0]], "ids": ["in"]}));
    let arc = run(s, "sketch.arc.center_point", json!({"center": [300, 100], "start": [300, 0], "sweep": 90}));
    run(s, "sketch.line", json!({"points": [[400, 100], [400, 500]], "ids": ["out"]}));
    run(s, "sketch.finish", json!({}));
    arc["created"]["curves"][0].as_str().unwrap().to_string()
}

const QUARTER: f64 = 50.0 * std::f64::consts::PI;

/// A bent member's volume, measured from its faceted mesh: within 0.1 %.
fn near_bent(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= want.abs() * 1e-3, "{what}: {got} ≠ {want}");
}

/// Lines and an arc that run on into each other make one bent member: the profile swept along
/// them (the tube's area times the centreline, as its centroid lies on it), keyed by the first
/// line. The cut list gives its length along the centreline, its bend as a bender makes it and its
/// cut length with the end allowance at both ends; without an allowance, an arc running to an end
/// is pointed out.
#[test]
fn lines_and_arcs_running_on_make_one_bent_member() {
    let mut s = setup();
    let arc = bent_rail(&mut s);
    let r = member(&mut s, &["in", &arc, "out"], json!({"path_sketch": "Rail", "end_allowance": "50 mm"})).unwrap();
    assert_eq!(r["bodies"], json!(["Rail"]), "{r}");
    let developed = 300.0 + QUARTER + 400.0;
    near_bent(measure(&s, "Rail").volume, AREA * developed, "volume");
    let w = &s.world_state().welds[0];
    assert_eq!(w.line, "in");
    let rows = cut_list(&mut s, json!({}));
    near(rows[0]["length_mm"].as_f64().unwrap(), developed, "length");
    near(rows[0]["cut_length_mm"].as_f64().unwrap(), developed + 100.0, "cut length");
    let bends = rows[0]["bends"].as_array().unwrap();
    assert_eq!(bends.len(), 1);
    near(bends[0]["feed_mm"].as_f64().unwrap(), 300.0, "feed");
    near(bends[0]["angle_deg"].as_f64().unwrap(), 90.0, "angle");
    near(bends[0]["radius_mm"].as_f64().unwrap(), 100.0, "radius");
    near(rows[0]["tail_mm"].as_f64().unwrap(), 400.0, "tail");
    // An arc alone is rolled: it bends right to both its ends.
    let r = member(&mut s, &[&arc], json!({"path_sketch": "Rail", "body_name": "Arc", "join_earlier": false})).unwrap();
    assert!(r["warning"].as_str().is_some_and(|w| w.contains("bent right to its end")), "{r}");
    near_bent(measure(&s, "Arc").volume, AREA * QUARTER, "arc volume");
}

/// Without `merge_arcs` each arc is a member of its own, and the lines meet it square (they run
/// on in line). A member ending on a bent member's straight run butts against it; one ending on
/// its bend butts flat against the plane touching its face there.
#[test]
fn bent_members_apart_and_members_ending_on_them() {
    let mut s = setup();
    let arc = bent_rail(&mut s);
    let r = member(&mut s, &["in", &arc, "out"], json!({"path_sketch": "Rail", "merge_arcs": false})).unwrap();
    assert_eq!(r["bodies"].as_array().unwrap().len(), 3, "{r}");
    near(measure(&s, "Rail").volume, AREA * 300.0, "first line");
    near_bent(measure(&s, "Rail (2)").volume, AREA * QUARTER, "arc");
    let mut s = setup();
    let arc = bent_rail(&mut s);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Braces"}));
    run(&mut s, "sketch.line", json!({"points": [[150, -300], [150, 0]], "ids": ["onto_run"]}));
    run(&mut s, "sketch.line", json!({"points": [[TO_BEND[0], -300], TO_BEND], "ids": ["onto_bend"]}));
    run(&mut s, "sketch.finish", json!({}));
    let r = member(
        &mut s,
        &["in", &arc, "out"],
        json!({"path_sketch": "Rail", "groups": [{"path_sketch": "Braces", "path": ["onto_run", "onto_bend"]}]}),
    )
    .unwrap();
    assert!(r["warning"].as_str().is_none_or(|w| !w.contains("bend of")), "{r}");
    // The brace onto the run stops at the tube's face, 25 short of the line.
    let rows = cut_list(&mut s, json!({"bodies": ["Rail (2)"]}));
    near(rows[0]["length_mm"].as_f64().unwrap(), 275.0, "brace onto the run");
    // The brace onto the bend meets the plane touching the tube there at 45°.
    let rows = cut_list(&mut s, json!({"bodies": ["Rail (3)"]}));
    let (_, length, c1, _, c2, a2) = row(&rows[0]);
    assert_eq!((c1.as_str(), c2.as_str()), ("square", "mitre"));
    near(length, ONTO_BEND, "brace onto the bend");
    near(a2, 45.0, "its end's angle");
}

/// The point of `bent_rail`'s bend halfway round, where its direction is 45° to X and Y.
const TO_BEND: [f64; 2] = [300.0 + 100.0 * std::f64::consts::FRAC_1_SQRT_2, 100.0 - 100.0 * std::f64::consts::FRAC_1_SQRT_2];

/// A tube from y = −300 up to `TO_BEND`, butted against the outside of the bend: from its start
/// to the line, back to the face (25 off the line, 45° across: 25·√2 short of it), then on to its
/// long point (half its width across the 45° cut: 25).
const ONTO_BEND: f64 = 300.0 + 100.0 - 100.0 * std::f64::consts::FRAC_1_SQRT_2 - 25.0 * std::f64::consts::SQRT_2 + 25.0;

/// An end cap fits the far end of a bent member, turned with the bend, and a gusset stands on its
/// straight run where another member meets it.
#[test]
fn bent_members_in_later_features() {
    let mut s = setup();
    let arc = bent_rail(&mut s);
    member(&mut s, &["in", &arc, "out"], json!({"path_sketch": "Rail"})).unwrap();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Post"}));
    run(&mut s, "sketch.line", json!({"points": [[150, -400], [150, 0]], "ids": ["post"]}));
    run(&mut s, "sketch.finish", json!({}));
    member(&mut s, &["post"], json!({"path_sketch": "Post", "body_name": "Post"})).unwrap();
    run(&mut s, "weld.end_cap", json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5}));
    let cap = measure(&s, "End Cap");
    near(cap.bbox.min.y, 500.0, "on the end, past it");
    near(cap.bbox.max.y, 505.0, "its thickness");
    near(cap.bbox.max.x - cap.bbox.min.x, 50.0, "the tube's width across");
    run(&mut s, "weld.gusset", json!({"members": ["Rail", "Post"], "size": 50, "thickness": 5}));
    near(measure(&s, "Gusset").volume, 0.5 * 50.0 * 50.0 * 5.0, "gusset on the straight run");
    // A ring has no straight run to stand on.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hoop"}));
    run(&mut s, "sketch.circle.center", json!({"center": [150, -200], "radius": 100, "id": "hoop"}));
    run(&mut s, "sketch.finish", json!({}));
    member(&mut s, &["hoop"], json!({"path_sketch": "Hoop", "body_name": "Hoop", "join_earlier": false})).unwrap();
    let e = s.execute("weld.gusset", &json!({"members": ["Hoop", "Post"], "size": 50, "thickness": 5})).unwrap_err().to_string();
    assert!(e.contains("straight run"), "{e}");
    let e = s.execute("weld.end_cap", &json!({"ends": [{"member": "Hoop", "end": "start"}], "thickness": 5})).unwrap_err().to_string();
    assert!(e.contains("ring"), "{e}");
}

/// Trim/Extend moves a bent member's end along its straight run (cut back within it, lengthened
/// past it) and refuses to cut back into the bend; a bent member is a boundary too, flat on the
/// plane touching its bend, never coped there.
#[test]
fn bent_members_trimmed_and_as_boundaries() {
    let mut s = setup();
    let arc = bent_rail(&mut s);
    member(&mut s, &["in", &arc, "out"], json!({"path_sketch": "Rail"})).unwrap();
    let developed = 300.0 + QUARTER + 400.0;
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [100, 0, 0], "normal": [1, 0, 0]}})).unwrap();
    near_bent(measure(&s, "Rail").volume, AREA * (developed - 100.0), "cut back at the start");
    trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [0, 600, 0], "normal": [0, 1, 0]}})).unwrap();
    near_bent(measure(&s, "Rail").volume, AREA * developed, "lengthened at the end");
    let rows = cut_list(&mut s, json!({"bodies": ["Rail"]}));
    near(rows[0]["length_mm"].as_f64().unwrap(), developed, "length");
    near(rows[0]["bends"][0]["feed_mm"].as_f64().unwrap(), 200.0, "feed to the bend");
    near(rows[0]["tail_mm"].as_f64().unwrap(), 500.0, "tail");
    let slant = std::f64::consts::FRAC_1_SQRT_2;
    let e = trim(&mut s, json!({"members": ["Rail"], "plane": {"origin": [330, 0, 0], "normal": [slant, slant, 0]}})).unwrap_err();
    assert!(e.contains("into its bend"), "{e}");
    // A straight member trimmed to the bend.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Spoke"}));
    run(&mut s, "sketch.line", json!({"points": [[TO_BEND[0], -300], [TO_BEND[0], -100]], "ids": ["spoke"]}));
    run(&mut s, "sketch.finish", json!({}));
    member(&mut s, &["spoke"], json!({"path_sketch": "Spoke", "body_name": "Spoke"})).unwrap();
    let e = trim(&mut s, json!({"members": ["Spoke"], "to": ["Rail"], "cope": true})).unwrap_err();
    assert!(e.contains("where it bends"), "{e}");
    trim(&mut s, json!({"members": ["Spoke"], "to": ["Rail"]})).unwrap();
    let rows = cut_list(&mut s, json!({"bodies": ["Spoke"]}));
    near(rows[0]["length_mm"].as_f64().unwrap(), ONTO_BEND, "spoke onto the bend");
}

/// A spline makes a member formed to its shape (no bender steps); a circle makes a ring, a
/// member ending on it butting against its face; a 3D sketch curve included from a model's round
/// edge is that arc, so a ring along it bends like one drawn.
#[test]
fn free_form_curves_rings_and_model_edges() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Shapes"}));
    let wave = run(&mut s, "sketch.spline.fit_point", json!({"points": [[0, 200], [100, 250], [200, 200], [300, 250]]}));
    let wave = wave["curves"][0].as_str().unwrap().to_string();
    run(&mut s, "sketch.circle.center", json!({"center": [500, 500], "radius": 100, "id": "hoop"}));
    run(&mut s, "sketch.line", json!({"points": [[500, 300], [500, 400]], "ids": ["stem"]}));
    run(&mut s, "sketch.finish", json!({}));
    member(&mut s, &[&wave, "hoop", "stem"], json!({"path_sketch": "Shapes"})).unwrap();
    let rows = cut_list(&mut s, json!({"bodies": ["Rail"]}));
    assert_eq!(rows[0]["formed"], true, "{rows:?}");
    assert_eq!(rows[0]["bends"], json!([]));
    let wave = rows[0]["length_mm"].as_f64().unwrap();
    assert!(wave > 300.0 && wave < 400.0, "{wave}");
    near_bent(measure(&s, "Rail").volume, AREA * wave, "formed member");
    let rows = cut_list(&mut s, json!({"bodies": ["Rail (2)"]}));
    assert_eq!((rows[0]["ring"].clone(), rows[0]["formed"].clone()), (json!(true), json!(false)));
    let round = 200.0 * std::f64::consts::PI;
    near(rows[0]["length_mm"].as_f64().unwrap(), round, "ring");
    near(rows[0]["bends"][0]["angle_deg"].as_f64().unwrap(), 360.0, "one turn");
    near_bent(measure(&s, "Rail (2)").volume, AREA * round, "ring volume");
    // The stem ends at the ring's bottom, so it stops at the tube's face 25 below.
    near(cut_list(&mut s, json!({"bodies": ["Rail (3)"]}))[0]["length_mm"].as_f64().unwrap(), 75.0, "stem");

    let mut s = setup();
    run(&mut s, "solid.cylinder", json!({"radius": 200, "height": 100, "body_name": "Drum"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Edge"}));
    // The rim is two half circles: they run on into one another as one ring.
    let r = run(&mut s, "sketch.include_3d", json!({"refs": [{"edge": [200, 0, 100]}, {"edge": [-200, 0, 100]}]}));
    run(&mut s, "sketch.finish", json!({}));
    let rim: Vec<&str> = r["created"]["wires"].as_array().unwrap_or_else(|| panic!("{r}")).iter().filter_map(Value::as_str).collect();
    assert_eq!(rim.len(), 2, "{r}");
    member(&mut s, &rim, json!({"path_sketch": "Edge", "body_name": "Rim"})).unwrap();
    let rows = cut_list(&mut s, json!({"bodies": ["Rim"]}));
    assert_eq!((rows[0]["ring"].clone(), rows[0]["formed"].clone()), (json!(true), json!(false)));
    near(rows[0]["bends"][0]["radius_mm"].as_f64().unwrap(), 200.0, "the edge's radius");
    near(rows[0]["length_mm"].as_f64().unwrap(), 400.0 * std::f64::consts::PI, "round the rim");
}

fn cut_list(s: &mut Session, p: Value) -> Vec<Value> {
    run(s, "weld.cut_list", p)["rows"].as_array().unwrap().clone()
}

/// One cut-list row as (qty, length, end1 cut, end1 angle, end2 cut, end2 angle).
fn row(r: &Value) -> (u64, f64, String, f64, String, f64) {
    let end = |e: &Value| (e["cut"].as_str().unwrap().to_string(), e["angle_deg"].as_f64().unwrap_or(0.0));
    let ((c1, a1), (c2, a2)) = (end(&r["end1"]), end(&r["end2"]));
    (r["qty"].as_u64().unwrap(), r["length_mm"].as_f64().unwrap(), c1, a1, c2, a2)
}

/// The mitred rectangle's cut list: 2 × 1000 and 2 × 600 at their long points, 45° both ends.
/// A mitre takes off as much as its long point adds, so in Steel a 1000 mm member weighs its
/// centre line's worth: 564 mm² × 950 mm × 7.85 g/cm³ = 4.20603 kg.
#[test]
fn the_mitred_rectangle_cut_list() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    run(&mut s, "material.assign", json!({"bodies": ["Rail", "Rail (2)", "Rail (3)", "Rail (4)"], "material": "Steel"}));
    let rows = cut_list(&mut s, json!({}));
    assert_eq!(rows.len(), 2, "{rows:?}");
    let (qty, length, c1, a1, c2, a2) = row(&rows[0]);
    assert_eq!((qty, c1.as_str(), c2.as_str()), (2, "mitre", "mitre"));
    near(length, 1000.0, "long rails");
    near(a1, 45.0, "start angle");
    near(a2, 45.0, "end angle");
    near(row(&rows[1]).1, 600.0, "short rails");
    assert_eq!(rows[0]["description"], "Tube");
    assert_eq!(rows[0]["members"], json!(["Rail", "Rail (3)"]));
    assert_eq!(rows[0]["material"], "Steel");
    near(rows[0]["mass_kg"].as_f64().unwrap(), 4.20603, "mass");
    let list = run(&mut s, "weld.cut_list", json!({}));
    near(list["stock"][0]["total_length_mm"].as_f64().unwrap(), 3200.0, "stock");
    assert_eq!(list["members"], 4);
}

/// The butted rectangle: 2 × 1000 and 2 × 500, square both ends.
#[test]
fn the_butted_rectangle_cut_list() {
    let mut s = setup();
    member(&mut s, &["l1", "l3", "l2", "l4"], json!({"corner": "butt"})).unwrap();
    let rows: Vec<_> = cut_list(&mut s, json!({})).iter().map(row).collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    for ((qty, length, c1, _, c2, _), want) in rows.iter().zip([1000.0, 500.0]) {
        assert_eq!((*qty, c1.as_str(), c2.as_str()), (2, "square", "square"));
        near(*length, want, "length");
    }
}

/// Four identical legs are one row; a diagonal ending on two rails has one angle each end;
/// a member consumed by a later feature leaves the list.
#[test]
fn identical_members_share_a_row() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "Legs"}));
    for (i, x) in [0, 300, 600, 900].iter().enumerate() {
        run(&mut s, "sketch.line", json!({"points": [[x, 0], [x, -700]], "ids": [format!("leg{i}")]}));
    }
    run(&mut s, "sketch.line", json!({"points": [[0, -100], [300, -500]], "ids": ["diag"]}));
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Legs", "path": ["leg0", "leg1", "leg2", "leg3", "diag"], "body_name": "L"});
    run(&mut s, "weld.member", p);
    let rows = cut_list(&mut s, json!({}));
    assert_eq!(rows.len(), 2, "{rows:?}");
    let (qty, length, ..) = row(&rows[0]);
    assert_eq!(qty, 4);
    near(length, 700.0, "legs");
    // The diagonal runs 300 across and 400 down, between the faces of legs 0 and 1: each end
    // is cut flat against a leg, atan(400 / 300) = 53.13° from square.
    let (qty, _, c1, a1, c2, a2) = row(&rows[1]);
    assert_eq!((qty, c1.as_str(), c2.as_str()), (1, "mitre", "mitre"));
    near(a1, 53.130_102_354_155_98, "diagonal start");
    near(a2, 53.130_102_354_155_98, "diagonal end");
    run(&mut s, "solid.combine", json!({"target": "L", "tools": ["L (2)"]}));
    assert_eq!(row(&cut_list(&mut s, json!({}))[0]).0, 3, "a combined leg is no longer cut");
}

/// A square tube turned 45° on a mitred corner is cut at a compound angle: atan(1/√2) = 35.26°
/// about each of the profile's axes.
#[test]
fn a_turned_profile_has_compound_ends() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({"angle": "45 deg"})).unwrap();
    let rows = cut_list(&mut s, json!({}));
    let e = &rows[0]["end1"];
    assert_eq!(e["cut"], "compound", "{e}");
    let want = (0.5f64.sqrt()).atan().to_degrees();
    near(e["mitre_deg"].as_f64().unwrap(), want, "mitre");
    near(e["bevel_deg"].as_f64().unwrap(), want, "bevel");
}

/// A member mitred the same way at both ends (the middle of a Z) is a parallelogram, not the
/// trapezoid of a rectangle's side: the same length and angles, but a different part.
#[test]
fn a_parallelogram_is_not_a_trapezoid() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Zed"}));
    run(&mut s, "sketch.line", json!({"points": [[2000, 0], [2500, 0], [2500, 550], [3000, 550]], "ids": ["z1", "z2", "z3"]}));
    run(&mut s, "sketch.finish", json!({}));
    member(&mut s, &["z1", "z2", "z3"], json!({"path_sketch": "Zed", "body_name": "Z"})).unwrap();
    let rows = cut_list(&mut s, json!({}));
    let by: Vec<(u64, f64, Value)> =
        rows.iter().map(|r| (r["qty"].as_u64().unwrap(), r["length_mm"].as_f64().unwrap(), r["members"].clone())).collect();
    assert_eq!(rows.len(), 4, "{by:?}");
    near(by[1].1, 600.0, "the rectangle's sides");
    assert_eq!(by[1].2, json!(["Rail (2)", "Rail (4)"]));
    near(by[3].1, 600.0, "the Z's middle");
    assert_eq!(by[3].2, json!(["Z (2)"]));
    assert_eq!(by[2].2, json!(["Z", "Z (3)"]), "the Z's ends are the same part, end for end");
    assert!(rows.iter().all(|r| r["mirror_of"].is_null()), "nothing here is handed");
}

/// Two 3D skeletons, one the other's mirror image: each middle member is mitred into the plane
/// of one neighbour at one end and of the other at its other end, so it is handed. The ends
/// match their mirrored partners as they are.
fn handed_frames(s: &mut Session, profile: &str) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "Handed"}));
    run(s, "sketch.options", json!({"sketch_3d": true}));
    let mut ids = Vec::new();
    for (x, z) in [(0, 400), (2000, -400)] {
        let r = run(s, "sketch.line3d", json!({"points": [[x, 0, 0], [x + 500, 0, 0], [x + 500, 300, 0], [x + 500, 300, z]]}));
        ids.push(r["curves"].clone());
    }
    run(s, "sketch.finish", json!({}));
    for (ids, name) in ids.iter().zip(["A", "B"]) {
        let p = json!({"sketch": profile, "profiles": [{"point": [23.5, 0]}], "path_sketch": "Handed", "path": ids, "body_name": name});
        run(s, "weld.member", p);
    }
}

fn handed_rows(s: &mut Session, p: Value) -> Vec<(u64, Value, Value, Value)> {
    cut_list(s, p).iter().map(|r| (r["qty"].as_u64().unwrap(), r["members"].clone(), r["mirror_of"].clone(), r["mirrored"].clone())).collect()
}

/// Mirror images are separate rows naming each other, or with `mirrors: together` one row that
/// lists the mirrored members.
#[test]
fn mirror_images_are_separate_rows_by_default() {
    let mut s = setup();
    handed_frames(&mut s, "Tube");
    let rows = handed_rows(&mut s, json!({}));
    assert_eq!(
        rows,
        vec![
            (2, json!(["A", "B"]), Value::Null, json!([])),
            (1, json!(["A (2)"]), json!(4), json!([])),
            (2, json!(["A (3)", "B (3)"]), Value::Null, json!([])),
            (1, json!(["B (2)"]), json!(2), json!([])),
        ]
    );
    let rows = handed_rows(&mut s, json!({"mirrors": "together"}));
    assert_eq!(rows[1], (2, json!(["A (2)", "B (2)"]), Value::Null, json!(["B (2)"])));
    assert_eq!(rows.len(), 3);
    let csv = run(&mut s, "weld.export_cut_list", json!({}))["text"].as_str().unwrap().to_string();
    assert!(csv.lines().next().unwrap().ends_with(",members,mirror_of,mirrored,profile"), "{csv}");
    assert!(csv.lines().nth(2).unwrap().ends_with(",\"A (2)\",4,\"\",\"Tube\""), "{csv}");
    assert!(s.execute("weld.cut_list", &json!({"mirrors": "sideways"})).is_err());
}

/// A round tube looks the same turned any way about its axis, but a handed member is still
/// handed.
#[test]
fn round_tube_mirror_images() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Pipe"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 25}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 22}));
    run(&mut s, "sketch.finish", json!({}));
    handed_frames(&mut s, "Pipe");
    let mirror_of: Vec<Value> = handed_rows(&mut s, json!({})).into_iter().map(|r| r.2).collect();
    assert_eq!(mirror_of, vec![Value::Null, json!(4), Value::Null, json!(2)]);
}

/// The export: CSV with one line per row, or versioned JSON; a file when given a path.
#[test]
fn the_cut_list_exports_as_csv_and_json() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    let csv = run(&mut s, "weld.export_cut_list", json!({}));
    let text = csv["text"].as_str().unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("item,qty,description,length_mm,end1,end1_mitre_deg"), "{text}");
    let mass = format!("{:.4}", AREA * 950.0 * solidcraft_doc::DEFAULT_DENSITY / 1e6);
    assert_eq!(
        lines[1],
        format!("1,2,\"Tube\",1000.000,mitre,45.0000,0.0000,mitre,45.0000,0.0000,{mass},\"Default\",\"Rail;Rail (3)\",,\"\",\"Tube\"")
    );
    let json = run(&mut s, "weld.export_cut_list", json!({"format": "json"}));
    let v: Value = serde_json::from_str(json["text"].as_str().unwrap()).unwrap();
    assert_eq!(v["schema"], "solidcraft-cut-list/1");
    assert_eq!(v["rows"].as_array().unwrap().len(), 2);
    let dir = std::env::temp_dir().join(format!("solidcraft-cut-list-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cuts.csv");
    run(&mut s, "weld.export_cut_list", json!({"path": path}));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_dir_all(&dir).ok();
    for bad in [json!({"format": "xml"}), json!({"bodies": ["nope"]}), json!({"length_tolerance": -1})] {
        assert!(s.execute("weld.export_cut_list", &bad).is_err(), "{bad} was accepted");
    }
}

/// Each member's body, by its skeleton line.
fn by_line(s: &Session) -> Vec<(String, String)> {
    s.world_state().welds.iter().map(|w| (w.line.clone(), w.body.clone())).collect()
}

/// Removing, reordering and adding skeleton lines renames no other member: a name (and the
/// material given to it) stays with its line, and a new line takes a number not used before.
#[test]
fn member_names_stay_with_their_skeleton_lines() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    run(&mut s, "material.assign", json!({"bodies": ["Rail (3)"], "material": "Steel"}));
    let named = |pairs: &[(&str, &str)]| pairs.iter().map(|(l, b)| (l.to_string(), b.to_string())).collect::<Vec<_>>();
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"path": ["l1", "l3", "l4"]}}));
    assert_eq!(by_line(&s), named(&[("l1", "Rail"), ("l3", "Rail (3)"), ("l4", "Rail (4)")]));
    assert_eq!(s.doc.materials.get("Rail (3)").map(String::as_str), Some("Steel"));
    near(measure(&s, "Rail (3)").volume, AREA * 950.0, "Rail (3) is still the top rail");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"path": ["l4", "l3", "l1"]}}));
    assert_eq!(by_line(&s), named(&[("l4", "Rail (4)"), ("l3", "Rail (3)"), ("l1", "Rail")]));
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"path": ["l2", "l4", "l3", "l1"]}}));
    assert_eq!(by_line(&s), named(&[("l2", "Rail (2)"), ("l4", "Rail (4)"), ("l3", "Rail (3)"), ("l1", "Rail")]));
    // A renamed member keeps its name too.
    run(&mut s, "body.rename", json!({"body": "Rail (4)", "name": "Left stile"}));
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"path": ["l4", "l1"]}}));
    assert_eq!(by_line(&s), named(&[("l4", "Left stile"), ("l1", "Rail")]));
    // Saved and reopened, the names are the same.
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert_eq!(by_line(&reopened), by_line(&s));
}

/// The dialog's edit (a redefine) keeps names by line as well, and members are called `Member`
/// when no name is given.
#[test]
fn a_redefined_member_keeps_its_names() {
    let mut s = setup();
    let p = |path: &[&str]| json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Frame", "path": path});
    run(&mut s, "weld.member", p(&["l1", "l2", "l3"]));
    run(&mut s, "timeline.redefine", json!({"feature": "StructuralMember1", "command": "weld.member", "params": p(&["l3", "l1"])}));
    assert_eq!(by_line(&s), vec![("l3".to_string(), "Member (3)".to_string()), ("l1".to_string(), "Member".to_string())]);
}

/// A second sketch `Cross` with a stretcher across the rails of the rectangle, centre line to
/// centre line, and a post down from its first corner.
fn cross(s: &mut Session) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "Cross"}));
    run(s, "sketch.line", json!({"points": [[475, 0], [475, 550]], "ids": ["x1"]}));
    run(s, "sketch.line", json!({"points": [[0, 0], [0, -300]], "ids": ["x2"]}));
    run(s, "sketch.finish", json!({}));
}

/// Members of a later feature ending on an earlier one's members butt against their faces (with
/// the gap); the earlier members are left whole. Turned off, they run to their line's ends.
#[test]
fn a_later_feature_butts_against_earlier_members() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    cross(&mut s);
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Cross", "path": ["x1", "x2"], "body_name": "Cross"});
    run(&mut s, "weld.member", p.clone());
    // Across the rails, inside face to inside face.
    near(measure(&s, "Cross").volume, AREA * 500.0, "stretcher");
    // From under the corner, where l1 starts and l4 ends: it butts against l1 (l4 runs in line).
    near(measure(&s, "Cross (2)").volume, AREA * 275.0, "post");
    for (name, length) in [("Rail", 950.0), ("Rail (3)", 950.0)] {
        near(measure(&s, name).volume, AREA * length, name);
    }
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember2", "set": {"gap": "2"}}));
    near(measure(&s, "Cross").volume, AREA * 496.0, "with the gap");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember2", "set": {"gap": null, "join_earlier": false}}));
    near(measure(&s, "Cross").volume, AREA * 550.0, "not joined");
    near(measure(&s, "Cross (2)").volume, AREA * 300.0, "post not joined");
    let mut q = p;
    q["join_earlier"] = json!("yes");
    assert!(s.execute("weld.member", &q).unwrap_err().to_string().contains("join_earlier"));
}

/// The workbench example: stretchers drawn leg centre to leg centre, in a feature of their own,
/// butt against the legs' faces (SHS 50, so 25 mm in at each end), corners included.
#[test]
fn the_workbench_stretchers_butt_against_the_legs() {
    let mut s = Session::default();
    let script: Value = serde_json::from_str(include_str!("../../../../examples/weldment-workbench.json")).unwrap();
    s.run_script(&script).unwrap();
    assert!(s.model.results.iter().all(|r| r.error.is_none()));
    const STRETCHER: f64 = 40.0 * 40.0 - 35.0 * 35.0;
    for (name, length) in [("Stretcher", 1150.0), ("Stretcher (2)", 1150.0), ("Stretcher (3)", 550.0), ("Stretcher (4)", 550.0)] {
        near(measure(&s, name).volume, STRETCHER * length, name);
    }
    // Under each top corner, a 120 mm gusset 6 mm thick with its corner clipped 10 mm.
    for name in ["Gusset", "Gusset (2)", "Gusset (3)", "Gusset (4)"] {
        near(measure(&s, name).volume, (120.0 * 120.0 / 2.0 - 50.0) * 6.0, name);
    }
}

/// The located point lies on the line: a corner of the tube's box puts a face on the line, a
/// point given in the profile's coordinates puts that point there; the turn is about it.
#[test]
fn the_profile_lies_on_the_line_at_its_located_point() {
    let mut s = setup();
    member(&mut s, &["l1"], json!({"locate": "top_left"})).unwrap();
    let bbox = |s: &Session| {
        let b = measure(s, "Rail").bbox;
        [b.min.y, b.max.y, b.min.z, b.max.z]
    };
    let close = |got: [f64; 4], want: [f64; 4], what: &str| got.iter().zip(want).for_each(|(g, w)| near(*g, w, what));
    // Profile x runs along world Y and its y up world Z: the top left corner sits on the line.
    close(bbox(&s), [0.0, 50.0, -50.0, 0.0], "top left");
    near(measure(&s, "Rail").volume, AREA * 950.0, "volume");
    for (locate, want) in [
        (json!("bottom"), [-25.0, 25.0, 0.0, 50.0]),
        (json!("right"), [-50.0, 0.0, -25.0, 25.0]),
        (json!({"point": [25, 0]}), [-50.0, 0.0, -25.0, 25.0]),
        (json!("centroid"), [-25.0, 25.0, -25.0, 25.0]),
    ] {
        run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"locate": locate}}));
        close(bbox(&s), want, &locate.to_string());
    }
    // A quarter turn about the top left corner swings the tube over to the other side.
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"locate": "top_left", "angle": "90 deg"}}));
    close(bbox(&s), [0.0, 50.0, 0.0, 50.0], "turned about the corner");
    for bad in [json!("upper_left"), json!({"point": [1e300, 0]}), json!({"point": "corner"})] {
        assert!(member(&mut s, &["l3"], json!({"locate": bad})).is_err(), "{bad}");
    }
    assert!(member(&mut s, &["l3"], json!({"mirror": "sideways"})).is_err());
}

/// An unequal angle 60 × 40 × 5 (no symmetry) on the YZ plane, its corner at the origin.
fn unequal_angle(s: &mut Session) {
    run(s, "sketch.create", json!({"plane": "YZ", "name": "Angle"}));
    run(s, "sketch.line", json!({"points": [[0, 0], [60, 0], [60, 5], [5, 5], [5, 40], [0, 40]], "closed": true}));
    run(s, "sketch.finish", json!({}));
}

/// Mirroring is about the axes through the located point: the angle's legs point the other way,
/// the corner stays on the line.
#[test]
fn the_profile_mirrors_about_its_located_point() {
    let mut s = setup();
    unequal_angle(&mut s);
    let p = |extra: Value| {
        let mut p = json!({"sketch": "Angle", "profiles": [{"point": [2, 20]}], "path_sketch": "Frame", "path": ["l1"], "body_name": "L"});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    run(&mut s, "weld.member", p(json!({"locate": {"point": [0, 0]}})));
    let bbox = |s: &Session| {
        let b = measure(s, "L").bbox;
        [b.min.y, b.max.y, b.min.z, b.max.z]
    };
    let close = |got: [f64; 4], want: [f64; 4], what: &str| got.iter().zip(want).for_each(|(g, w)| near(*g, w, what));
    close(bbox(&s), [0.0, 60.0, 0.0, 40.0], "as drawn");
    let area = 60.0 * 5.0 + 35.0 * 5.0;
    for (mirror, want) in [("left_right", [-60.0, 0.0, 0.0, 40.0]), ("up_down", [0.0, 60.0, -40.0, 0.0])] {
        run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"mirror": mirror}}));
        close(bbox(&s), want, mirror);
        near(measure(&s, "L").volume, area * 950.0, mirror);
    }
    // About the box's left middle instead, left to right: the angle swings round that point.
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"locate": "left", "mirror": "left_right"}}));
    close(bbox(&s), [-60.0, 0.0, -20.0, 20.0], "about the left middle");
}

/// A profile with arcs mirrors exactly: a D (a half disc of radius 20 on a 40 mm flat).
#[test]
fn a_mirrored_profile_keeps_its_arcs() {
    let mut s = setup();
    let half = std::f64::consts::FRAC_PI_2;
    let d = json!({"outer": {"segs": [
        {"type": "line", "a": [0, 20], "b": [0, -20]},
        {"type": "arc", "center": [0, 0], "radius": 20, "start": -half, "sweep": 2.0 * half},
    ]}, "holes": []});
    let p = json!({"stock": {"name": "D", "region": d}, "path_sketch": "Frame", "path": ["l1"], "body_name": "D", "locate": "left"});
    run(&mut s, "weld.member", p);
    // Measured on the body's mesh: within its chord tolerance of the half disc's area.
    let drawn = measure(&s, "D").volume;
    let area = std::f64::consts::PI * 400.0 / 2.0;
    assert!((drawn - area * 950.0).abs() < area * 950.0 * 1e-3, "volume {drawn}");
    near(measure(&s, "D").bbox.max.y, 20.0, "round side out along Y");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"mirror": "left_right"}}));
    let m = measure(&s, "D");
    near(m.volume, drawn, "mirrored volume");
    near(m.bbox.min.y, -20.0, "round side the other way");
    near(m.bbox.max.y, 0.0, "flat on the line");
}

/// A frame of the same lines built with the angle mirrored: top to bottom, each member is the
/// first frame's turned over (one row each); left to right, the leg stands on the mitres' short
/// side instead of their long side, another part though each member's own leans are the same.
#[test]
fn a_mirrored_frame_in_the_cut_list() {
    let mut s = setup();
    unequal_angle(&mut s);
    for (name, extra) in [("A", json!({})), ("B", json!({"mirror": "up_down"}))] {
        let mut p =
            json!({"sketch": "Angle", "profiles": [{"point": [2, 20]}], "path_sketch": "Frame", "path": ["l1", "l2", "l3", "l4"], "body_name": name});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        run(&mut s, "weld.member", p);
    }
    let rows = cut_list(&mut s, json!({}));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r["qty"] == 4 && r["mirror_of"].is_null()), "{rows:?}");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember2", "set": {"mirror": "left_right"}}));
    let rows = cut_list(&mut s, json!({}));
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(rows.iter().all(|r| r["qty"] == 2 && r["mirror_of"].is_null()), "{rows:?}");
    assert_eq!(rows[0]["members"], json!(["A", "A (3)"]));
}

/// The design's stock lengths count the bars to buy: the longest pieces first, each into the
/// first bar with room (a kerf between pieces), each bar bought as short as its pieces allow;
/// a member longer than every length is listed to be spliced. Setting them is an undo step, and
/// lengths that can't be used are refused.
#[test]
fn stock_lengths_count_the_bars_to_buy() {
    let mut s = setup();
    member(&mut s, &["l1", "l2", "l3", "l4"], json!({})).unwrap();
    let stock = |s: &mut Session, p: Value| run(s, "weld.cut_list", p)["stock"][0].clone();
    let none = stock(&mut s, json!({}));
    assert_eq!(none["lengths_from"], Value::Null);
    assert_eq!(none["buy"], json!([]));
    // Mitred at the outside corners: two of 1000 and two of 600 at the long points.
    let r = run(&mut s, "weld.stock_lengths", json!({"profile": "Tube", "lengths": [2000, "3 m", 2000]}));
    assert_eq!(r["lengths_mm"], json!([3000.0, 2000.0]));
    let x = stock(&mut s, json!({"kerf": 3}));
    assert_eq!(x["lengths_from"], "design");
    assert_eq!(x["buy"], json!([{"length_mm": 3000.0, "qty": 1}, {"length_mm": 2000.0, "qty": 1}]));
    let bars = x["bars"].as_array().unwrap();
    near(bars[0]["drop_mm"].as_f64().unwrap(), 3000.0 - (1000.0 + 3.0 + 1000.0 + 3.0 + 600.0), "first drop");
    near(x["drop_mm"].as_f64().unwrap(), 394.0 + 1400.0, "drop");
    assert_eq!(x["too_long"], json!([]));
    // Bars too short for the long members: those are left to be spliced.
    run(&mut s, "weld.stock_lengths", json!({"profile": "Tube", "lengths": [800]}));
    let x = stock(&mut s, json!({"kerf": 3}));
    assert_eq!(x["too_long"], json!(["Rail", "Rail (3)"]));
    assert_eq!(x["buy"], json!([{"length_mm": 800.0, "qty": 2}]));
    let csv = run(&mut s, "weld.export_cut_list", json!({"kerf": 3}))["text"].as_str().unwrap().to_string();
    assert!(csv.contains("stock,bar,length_mm,items,cuts_mm,drop_mm") && csv.contains("too long"), "{csv}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.stock_lengths["Tube"], [3000.0, 2000.0]);
    run(&mut s, "weld.stock_lengths", json!({"profile": "Tube", "lengths": []}));
    assert!(s.doc.stock_lengths.is_empty());
    let many: Vec<f64> = (1..=17).map(f64::from).collect();
    for bad in [json!([-1]), json!([0]), json!(["Tube"]), json!(many), json!("6 m"), json!([1e9])] {
        assert!(s.execute("weld.stock_lengths", &json!({"profile": "Tube", "lengths": bad})).is_err(), "{bad} was taken");
    }
    assert!(s.execute("weld.stock_lengths", &json!({"profile": "", "lengths": [1000]})).is_err());
    assert!(s.execute("weld.cut_list", &json!({"kerf": -1})).is_err());
    assert!(s.doc.stock_lengths.is_empty());
}
