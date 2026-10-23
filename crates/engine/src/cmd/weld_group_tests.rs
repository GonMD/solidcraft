//! Structural Member groups: further skeleton lines from other sketches in one feature, joined to
//! its path as one, each with its own angle and corner. A square tube 50 × 50 × 3 (area 564 mm²),
//! as in `weld_tests`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::Session;

const AREA: f64 = 564.0;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// The tube profile `Tube`, the skeleton `Frame` (a line `l1` from the origin to 950 on X), and
/// sketches `Side` (a line also called `l1`, from Frame's end up 550 on Y; and `far`, apart) and
/// `Top` (`t`, from the origin up 550 on Y). Returns the ids of Side and Top.
fn setup() -> (Session, u64, u64) {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Tube"}));
    run(&mut s, "sketch.line", json!({"points": [[-25, -25], [25, -25], [25, 25], [-25, 25]], "closed": true}));
    run(&mut s, "sketch.line", json!({"points": [[-22, -22], [22, -22], [22, 22], [-22, 22]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Frame"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [950, 0]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    let side = run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Side"}))["sketch"].as_u64().unwrap();
    run(&mut s, "sketch.line", json!({"points": [[950, 0], [950, 550]], "ids": ["l1"]}));
    run(&mut s, "sketch.line", json!({"points": [[0, 1000], [500, 1000]], "ids": ["far"]}));
    run(&mut s, "sketch.finish", json!({}));
    let top = run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Top"}))["sketch"].as_u64().unwrap();
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, 550]], "ids": ["t"]}));
    run(&mut s, "sketch.finish", json!({}));
    (s, side, top)
}

fn member(s: &mut Session, extra: Value) -> Result<Value, String> {
    let mut p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Frame", "path": ["l1"], "body_name": "Rail"});
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    s.execute("weld.member", &p).map_err(|e| e.to_string())
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.world_state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap()
}

fn near(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-6, "{what}: {got} ≠ {want}");
}

fn by_line(s: &Session) -> Vec<(String, String)> {
    s.world_state().welds.iter().map(|w| (w.line.clone(), w.body.clone())).collect()
}

fn named(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// A line of another sketch (with the same id) in a group mitres with the path's line, as one
/// path would, and is keyed `2:l1`; separate features would only butt. Saved and reopened, the
/// same.
#[test]
fn a_group_from_another_sketch_mitres_with_the_path() {
    let (mut s, ..) = setup();
    member(&mut s, json!({"groups": [{"path_sketch": "Side", "path": ["l1"]}]})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 950.0, "rail");
    near(measure(&s, "Rail (2)").volume, AREA * 550.0, "side");
    near(measure(&s, "Rail").bbox.max.x, 975.0, "mitred outside corner");
    assert_eq!(by_line(&s), named(&[("l1", "Rail"), ("2:l1", "Rail (2)")]));
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    near(measure(&reopened, "Rail (2)").volume, AREA * 550.0, "reopened");
}

/// Members of two groups meet as the later group's corner says; a butt's earlier group runs
/// through. A joint by key turns it round.
#[test]
fn groups_meet_by_the_later_groups_corner() {
    let (mut s, ..) = setup();
    member(&mut s, json!({"groups": [{"path_sketch": "Side", "path": ["l1"], "corner": "butt"}]})).unwrap();
    near(measure(&s, "Rail").volume, AREA * 975.0, "rail runs through");
    near(measure(&s, "Rail (2)").volume, AREA * 525.0, "side butts");
    run(
        &mut s,
        "timeline.edit",
        json!({"feature": "StructuralMember1", "set": {"joints": [{"members": ["l1", "2:l1"], "corner": "butt", "through": "2:l1"}]}}),
    );
    near(measure(&s, "Rail").volume, AREA * 925.0, "rail butts");
    near(measure(&s, "Rail (2)").volume, AREA * 575.0, "side runs through");
}

/// A group's angle turns its members only; without one they take the feature's.
#[test]
fn a_group_turns_by_its_own_angle() {
    let (mut s, ..) = setup();
    member(&mut s, json!({"angle": "45 deg", "groups": [{"path_sketch": "Side", "path": ["far"], "angle": "0 deg"}]})).unwrap();
    let height = |s: &Session, b: &str| measure(s, b).bbox.max.z - measure(s, b).bbox.min.z;
    near(height(&s, "Rail"), 50.0 * 2f64.sqrt(), "feature's angle");
    near(height(&s, "Rail (2)"), 50.0, "group's angle");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"groups": [{"path_sketch": 3, "path": ["far"]}]}}));
    near(height(&s, "Rail (2)"), 50.0 * 2f64.sqrt(), "feature's angle again");
}

/// Removing a group renames no other member, and joints follow their members to their new keys
/// (a joint of a member gone is dropped).
#[test]
fn removing_a_group_keeps_names_and_joints_by_line() {
    let (mut s, side, top) = setup();
    let joints = json!([{"members": ["l1", "3:t"], "corner": "butt", "through": "3:t"}, {"members": ["l1", "2:l1"], "corner": "butt"}]);
    member(&mut s, json!({"groups": [{"path_sketch": side, "path": ["l1"]}, {"path_sketch": top, "path": ["t"]}], "joints": joints})).unwrap();
    assert_eq!(by_line(&s), named(&[("l1", "Rail"), ("2:l1", "Rail (2)"), ("3:t", "Rail (3)")]));
    near(measure(&s, "Rail (3)").volume, AREA * 575.0, "top runs through");
    run(&mut s, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"groups": [{"path_sketch": top, "path": ["t"]}]}}));
    assert_eq!(by_line(&s), named(&[("l1", "Rail"), ("2:t", "Rail (3)")]));
    let f = s.doc.features.iter().find(|f| f.name == "StructuralMember1").unwrap();
    let solidcraft_doc::FeatureKind::WeldMember { joints, .. } = &f.kind else { panic!("not a member") };
    assert_eq!(serde_json::to_value(joints).unwrap(), json!([{"members": ["l1", "2:t"], "corner": "butt", "through": "2:t"}]));
    near(measure(&s, "Rail (3)").volume, AREA * 575.0, "top still runs through");
    near(measure(&s, "Rail").volume, AREA * 925.0, "rail butts against it");
}

/// Groups are refused without lines, with a line another group has, and joints by keys that
/// aren't there.
#[test]
fn bad_groups_are_refused() {
    let (mut s, ..) = setup();
    let e = member(&mut s, json!({"groups": [{"path_sketch": "Side", "path": []}]})).unwrap_err();
    assert!(e.contains("group 2"), "{e}");
    let e = member(&mut s, json!({"groups": [{"path_sketch": "Side", "path": ["l1"]}], "joints": [{"members": ["l1", "3:l1"], "corner": "butt"}]}))
        .unwrap_err();
    assert!(e.contains("3:l1"), "{e}");
    let e = member(&mut s, json!({"groups": [{"path_sketch": "Frame", "path": ["l1"]}]})).unwrap_err();
    assert!(e.contains("`2:l1` lies on another member"), "{e}");
}
