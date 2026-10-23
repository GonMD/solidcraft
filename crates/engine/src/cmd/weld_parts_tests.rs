//! Weldment End Caps, Gussets and Weld Beads through the commands, on members of a square tube 50 × 50 × 3
//! (sharp corners), checked by volume and extent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// The tube profile `Tube` and a skeleton `Lines` on XY: `a` from the origin to (500, 0) and `b`
/// from the origin to (0, 400), made into `Rail` and `Leg`, mitred at the origin.
fn setup() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Tube"}));
    run(&mut s, "sketch.line", json!({"points": [[-25, -25], [25, -25], [25, 25], [-25, 25]], "closed": true}));
    run(&mut s, "sketch.line", json!({"points": [[-22, -22], [22, -22], [22, 22], [-22, 22]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Lines"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [500, 0]], "ids": ["a"]}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, 400]], "ids": ["b"]}));
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Lines", "path": ["a", "b"], "body_names": ["Rail", "Leg"]});
    run(&mut s, "weld.member", p);
    s
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.world_state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap()
}

fn near(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-6, "{what}: {got} ≠ {want}");
}

fn exec(s: &mut Session, cmd: &str, p: Value) -> Result<Value, String> {
    s.execute(cmd, &p).map_err(|e| e.to_string())
}

/// A cap on the rail's free end stands on it; set in, it is smaller; inside, it fills the hole
/// flush with the end. Picking the end face picks the same end.
#[test]
fn end_caps_close_a_tube() {
    let mut s = setup();
    let r = exec(&mut s, "weld.end_cap", json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": "5 mm", "name": "Caps"})).unwrap();
    assert_eq!(r["name"], "Caps");
    let cap = measure(&s, "End Cap");
    near(cap.volume, 50.0 * 50.0 * 5.0, "outside");
    near(cap.bbox.min.x, 500.0, "on the end");
    near(cap.bbox.max.x, 505.0, "standing out");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"inset": "3 mm"}}));
    near(measure(&s, "End Cap").volume, 44.0 * 44.0 * 5.0, "set in");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"inset": "0.5 mm", "inside": true}}));
    let cap = measure(&s, "End Cap");
    near(cap.volume, 43.0 * 43.0 * 5.0, "inside, with clearance");
    near(cap.bbox.min.x, 495.0, "into the tube");
    near(cap.bbox.max.x, 500.0, "flush");
    exec(&mut s, "weld.end_cap", json!({"faces": [{"body": "Leg", "point": [23.5, 400, 0]}], "thickness": 4})).unwrap();
    let cap = measure(&s, "End Cap (2)");
    near(cap.volume, 50.0 * 50.0 * 4.0, "picked by its face");
    near(cap.bbox.max.y, 404.0, "on the leg's end");
}

/// A cap on the rail's mitred start is the 45° end face's shape, 50 × 50√2, standing 5 off it
/// along the face's normal; set in, 44 × 44√2. Inside, a slice of the 43 × 43 hole between the end
/// face and a plane 5 into the tube.
#[test]
fn end_caps_close_a_mitred_end() {
    let mut s = setup();
    exec(&mut s, "weld.end_cap", json!({"ends": [{"member": "Rail", "end": "start"}], "thickness": 5, "name": "Caps"})).unwrap();
    let r2 = 2f64.sqrt();
    let cap = measure(&s, "End Cap");
    near(cap.volume, 50.0 * 50.0 * r2 * 5.0, "outside");
    // The face runs from (−25, −25) to (25, 25); the plate stands off it toward (−1, 1)/√2, away
    // from the rail.
    near(cap.bbox.max.x, 25.0, "the face's far corner");
    near(cap.bbox.min.x, -25.0 - 5.0 / r2, "standing off the face");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"inset": "3 mm"}}));
    near(measure(&s, "End Cap").volume, 44.0 * 44.0 * r2 * 5.0, "set in");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"inset": "0.5 mm", "inside": true}}));
    near(measure(&s, "End Cap").volume, 43.0 * 43.0 * r2 * 5.0, "inside");
}

/// A round tube (Ø30 × 3) cut at 45°: its cap is the ellipse the cut shows, π · 15 · 15√2, and
/// inside, a slice of its Ø24 hole (round plates are measured to about 1e-4).
#[test]
fn a_round_tube_cut_at_an_angle_is_capped() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Round"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 15}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 12}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Lines"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [300, 0]], "ids": ["bar"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "weld.member",
        json!({"sketch": "Round", "profiles": [{"point": [13.5, 0]}], "path_sketch": "Lines", "path": ["bar"], "body_name": "Bar"}),
    );
    run(&mut s, "weld.trim_extend", json!({"members": ["Bar"], "plane": {"origin": [300, 0, 0], "normal": [1, 1, 0]}}));
    exec(&mut s, "weld.end_cap", json!({"ends": [{"member": "Bar", "end": "end"}], "thickness": 4, "name": "Caps"})).unwrap();
    let r2 = 2f64.sqrt();
    let pi = std::f64::consts::PI;
    let v = measure(&s, "End Cap").volume;
    assert!((v - pi * 225.0 * r2 * 4.0).abs() < v * 2e-4, "outside {v}");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"inside": true}}));
    let v = measure(&s, "End Cap").volume;
    assert!((v - pi * 144.0 * r2 * 4.0).abs() < v * 2e-4, "inside {v}");
}

/// A 50 × 50 cap with its corners chamfered 5 loses four 12.5 triangles; filleted r5, four
/// 25 − 25π/4 corners. On the mitred end the corners are finished on the slanted face, so the
/// 50 × 50√2 plate loses the same. Inside, the 43 × 43 slice of the hole is chamfered 2.
#[test]
fn end_cap_corners_are_chamfered_or_filleted() {
    let mut s = setup();
    let p = json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "corner": "chamfer", "corner_size": 5, "name": "Caps"});
    exec(&mut s, "weld.end_cap", p).unwrap();
    near(measure(&s, "End Cap").volume, (2500.0 - 50.0) * 5.0, "chamfered");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"corner": "fillet"}}));
    let fillet = 100.0 - 25.0 * std::f64::consts::PI;
    let v = measure(&s, "End Cap").volume;
    assert!((v - (2500.0 - fillet) * 5.0).abs() < v * 1e-4, "filleted {v}");
    exec(&mut s, "weld.end_cap", json!({"ends": [{"member": "Rail", "end": "start"}], "thickness": 5, "corner": "fillet", "corner_size": 5}))
        .unwrap();
    let v = measure(&s, "End Cap (2)").volume;
    assert!((v - (2500.0 * 2f64.sqrt() - fillet) * 5.0).abs() < v * 1e-4, "mitred end {v}");
    run(&mut s, "timeline.edit", json!({"feature": "Caps", "set": {"inset": "0.5 mm", "inside": true, "corner": "chamfer", "corner_size": "2 mm"}}));
    near(measure(&s, "End Cap").volume, (43.0 * 43.0 - 8.0) * 5.0, "inside, chamfered");
    // The dialog's edit is a redefine.
    let p = json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "corner": "fillet", "corner_size": 5});
    run(&mut s, "timeline.redefine", json!({"feature": "Caps", "command": "weld.end_cap", "params": p}));
    let v = measure(&s, "End Cap").volume;
    assert!((v - (2500.0 - fillet) * 5.0).abs() < v * 1e-4, "redefined {v}");
}

#[test]
fn wrong_end_caps_are_refused() {
    let mut s = setup();
    for (p, want) in [
        (json!({"ends": [{"member": "Nope", "end": "end"}], "thickness": 5}), "not a structural member"),
        (json!({"ends": [{"member": "Rail", "end": "middle"}], "thickness": 5}), "start or end"),
        (json!({"ends": [], "thickness": 5}), "ends"),
        (json!({"ends": [{"member": "Rail", "end": "end"}]}), "thickness"),
        (json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "inset": 30}), "inset"),
        (json!({"ends": [{"member": "Rail", "end": "end"}, {"member": "Rail", "end": "end"}], "thickness": 5}), "twice"),
        (json!({"faces": [{"body": "Rail", "point": [250, 25, 0]}], "thickness": 5}), "end faces"),
        (json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "corner": "round", "corner_size": 2}), "square, chamfer or fillet"),
        (json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "corner": "fillet"}), "corner_size"),
        (json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "corner_size": 2}), "needs `corner`"),
        (json!({"ends": [{"member": "Rail", "end": "end"}], "thickness": 5, "corner": "chamfer", "corner_size": 30}), "too large"),
    ] {
        let e = exec(&mut s, "weld.end_cap", p.clone()).unwrap_err();
        assert!(e.contains(want), "{p}: {e}");
    }
    assert!(s.world_state().body("End Cap").is_none());
}

/// A triangular gusset in the corner of the L: legs along the members' inner faces from where
/// they meet (25, 25), centred on the skeleton's plane. Clipped and with square edges, its area
/// follows.
#[test]
fn a_gusset_fills_the_corner() {
    let mut s = setup();
    let r = exec(&mut s, "weld.gusset", json!({"members": ["Rail", "Leg"], "size": 100, "thickness": "6 mm", "name": "Corner plate"})).unwrap();
    assert_eq!(r["name"], "Corner plate");
    let g = measure(&s, "Gusset");
    near(g.volume, 5000.0 * 6.0, "triangle");
    near(g.bbox.min.x, 25.0, "on the leg's face");
    near(g.bbox.min.y, 25.0, "on the rail's face");
    near(g.bbox.max.x, 125.0, "along the rail");
    near(g.bbox.max.z - g.bbox.min.z, 6.0, "thickness");
    near(g.bbox.min.z, -3.0, "centred");
    run(&mut s, "timeline.edit", json!({"feature": "Corner plate", "set": {"clip": "10 mm"}}));
    near(measure(&s, "Gusset").volume, (5000.0 - 50.0) * 6.0, "clipped");
    run(&mut s, "timeline.edit", json!({"feature": "Corner plate", "set": {"clip": "0 mm", "edge": "20 mm", "size2": "100 mm"}}));
    near(measure(&s, "Gusset").volume, 6800.0 * 6.0, "with square edges");
    // The dialog's edit is a redefine.
    let p = json!({"members": ["Rail", "Leg"], "size": 80, "thickness": 6, "clip": 10});
    run(&mut s, "timeline.redefine", json!({"feature": "Corner plate", "command": "weld.gusset", "params": p}));
    near(measure(&s, "Gusset").volume, (3200.0 - 50.0) * 6.0, "redefined");
    run(&mut s, "timeline.edit", json!({"feature": "Corner plate", "set": {"size": "100 mm", "clip": "0 mm", "edge": "20 mm", "size2": "100 mm"}}));
    // The legs follow a longer rail tube: the gusset is rebuilt where the faces now are.
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    near(measure(&reopened, "Gusset").volume, 6800.0 * 6.0, "reopened");
}

/// The plate's thickness to one side of the skeleton's plane (+z: the rail's leg crossed with
/// the leg's) or the other, and offset. At a T, reversing the leg along the member that runs
/// through takes the other corner; at the L's end there is no member that way.
#[test]
fn a_gusset_is_placed_off_its_plane_and_in_the_other_corner() {
    let mut s = setup();
    let z = |s: &Session| {
        let g = measure(s, "Gusset");
        (g.bbox.min.z, g.bbox.max.z)
    };
    exec(&mut s, "weld.gusset", json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6, "side": "one", "name": "Plate"})).unwrap();
    assert_eq!(z(&s), (0.0, 6.0), "one side");
    run(&mut s, "timeline.edit", json!({"feature": "Plate", "set": {"side": "other", "offset": "-10 mm"}}));
    assert_eq!(z(&s), (-16.0, -10.0), "the other side, offset");
    near(measure(&s, "Gusset").volume, 5000.0 * 6.0, "the same plate");
    let e = exec(&mut s, "weld.gusset", json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6, "reverse_first": true})).unwrap_err();
    assert!(e.contains("doesn't run on past the joint"), "{e}");
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Upright"}));
    run(&mut s, "sketch.line", json!({"points": [[200, 0], [200, 300]], "ids": ["up"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "weld.member",
        json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Upright", "path": ["up"], "body_name": "Post"}),
    );
    exec(&mut s, "weld.gusset", json!({"members": ["Rail", "Post"], "size": 100, "thickness": 6, "name": "T"})).unwrap();
    let g = measure(&s, "Gusset (2)");
    assert_eq!((g.bbox.min.x, g.bbox.max.x), (225.0, 325.0), "toward the rail's far end");
    run(&mut s, "timeline.edit", json!({"feature": "T", "set": {"reverse_first": true}}));
    let g = measure(&s, "Gusset (2)");
    assert_eq!((g.bbox.min.x, g.bbox.max.x), (75.0, 175.0), "the other corner");
    near(g.bbox.min.y, 25.0, "on the rail's face");
}

/// Caps and gussets are listed as plates after the members' rows: the two caps on the square
/// ends are one row, the gusset another; the CSV has them as a second table.
#[test]
fn plates_are_in_the_cut_list() {
    let mut s = setup();
    exec(&mut s, "weld.end_cap", json!({"ends": [{"member": "Rail", "end": "end"}, {"member": "Leg", "end": "end"}], "thickness": 5})).unwrap();
    exec(&mut s, "weld.gusset", json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6})).unwrap();
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(list["rows"].as_array().unwrap().len(), 2, "{list}");
    let plates = list["plates"].as_array().unwrap();
    assert_eq!(plates.len(), 2, "{list}");
    assert_eq!(list["plate_count"], 3);
    let (cap, gusset) = (&plates[0], &plates[1]);
    assert_eq!((cap["item"].as_u64(), cap["qty"].as_u64(), cap["kind"].as_str()), (Some(3), Some(2), Some("end_cap")), "{cap}");
    assert_eq!(cap["bodies"], json!(["End Cap", "End Cap (2)"]));
    near(cap["thickness_mm"].as_f64().unwrap(), 5.0, "cap thickness");
    near(cap["width_mm"].as_f64().unwrap(), 50.0, "cap width");
    near(cap["area_mm2"].as_f64().unwrap(), 2500.0, "cap area");
    near(cap["mass_kg"].as_f64().unwrap(), measure(&s, "End Cap").volume * s.doc.density("End Cap") / 1e6, "cap mass");
    assert_eq!((gusset["item"].as_u64(), gusset["kind"].as_str()), (Some(4), Some("gusset")), "{gusset}");
    near(gusset["area_mm2"].as_f64().unwrap(), 5000.0, "gusset area");
    near(gusset["height_mm"].as_f64().unwrap(), 100.0, "gusset leg");
    let only = run(&mut s, "weld.cut_list", json!({"bodies": ["Gusset"]}));
    assert_eq!((only["rows"].as_array().unwrap().len(), only["plates"].as_array().unwrap().len()), (0, 1), "{only}");
    let csv = run(&mut s, "weld.export_cut_list", json!({}))["text"].as_str().unwrap().to_string();
    assert!(csv.contains("\r\n\r\nitem,qty,plate,thickness_mm,width_mm,height_mm,area_mm2,mass_kg,material,bodies,description\r\n"), "{csv}");
    assert!(csv.contains("3,2,end_cap,5.000,50.000,50.000,2500.00,"), "{csv}");
}

#[test]
fn wrong_gussets_are_refused() {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Far"}));
    run(&mut s, "sketch.line", json!({"points": [[1000, 100], [1000, 400]], "ids": ["c"]}));
    run(&mut s, "sketch.line", json!({"points": [[0, 200], [500, 200]], "ids": ["d"]}));
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Far", "path": ["c", "d"], "body_names": ["Far", "Shelf"], "join_earlier": false});
    run(&mut s, "weld.member", p);
    for (p, want) in [
        (json!({"members": ["Rail", "Far"], "size": 100, "thickness": 6}), "don't meet"),
        (json!({"members": ["Rail", "Shelf"], "size": 100, "thickness": 6}), "side by side"),
        (json!({"members": ["Rail", "Rail"], "size": 100, "thickness": 6}), "two different"),
        (json!({"members": ["Rail"], "size": 100, "thickness": 6}), "two structural members"),
        (json!({"members": ["Rail", "Leg"], "thickness": 6}), "size"),
        (json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6, "clip": 150}), "too large"),
        (json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6, "edge": 150}), "too large"),
        (json!({"members": ["Rail", "Leg"], "size": -1, "thickness": 6}), "size"),
        (json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6, "side": "up"}), "center, one or other"),
        (json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6, "offset": "1e9 mm"}), "offset"),
    ] {
        let e = exec(&mut s, "weld.gusset", p.clone()).unwrap_err();
        assert!(e.contains(want), "{p}: {e}");
    }
    assert!(s.world_state().body("Gusset").is_none());
}

/// `setup`'s L with a post butting onto the rail at x = 300 and a gusset in the corner.
fn welded() -> Session {
    let mut s = setup();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Upright"}));
    run(&mut s, "sketch.line", json!({"points": [[300, 0], [300, 300]], "ids": ["up"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "weld.member",
        json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Upright", "path": ["up"], "body_name": "Post"}),
    );
    run(&mut s, "weld.gusset", json!({"members": ["Rail", "Leg"], "size": 100, "thickness": 6}));
    s
}

fn bead<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["beads"].as_array().unwrap().iter().find(|b| b["name"] == name).unwrap_or_else(|| panic!("no bead {name}: {list}"))
}

/// Seams are found where the bodies meet: the mitre's outline (less the 6 mm the gusset covers at
/// its inner corner), the post's outline on the rail (not its bore), and the gusset's edges on the
/// rail (not its end against the leg). Intermittent welds count as many as fit along a seam.
#[test]
fn weld_beads_follow_the_seams() {
    let mut s = welded();
    run(&mut s, "weld.bead", json!({"bodies": ["Rail", "Leg"], "size": 5, "name": "Mitre weld"}));
    run(&mut s, "weld.bead", json!({"bodies": ["Post", "Rail"], "size": 4, "name": "Post weld", "field": true, "note": "GMAW"}));
    run(&mut s, "weld.bead", json!({"bodies": ["Gusset", "Rail"], "size": 3, "name": "Gusset weld", "length": 30, "pitch": 50}));
    // Nothing is added: the bodies are as they were.
    near(measure(&s, "Post").volume, 564.0 * 275.0, "post");
    let list = run(&mut s, "weld.beads", json!({}));
    let mitre = bead(&list, "Mitre weld");
    // Two 50 mm sides and two 50√2 diagonals, less the gusset's 6 mm in the corner.
    near(mitre["seam_mm"].as_f64().unwrap(), 100.0 + 100.0 * 2f64.sqrt() - 6.0, "mitre seam");
    near(mitre["volume_mm3"].as_f64().unwrap(), 12.5 * mitre["length_mm"].as_f64().unwrap(), "fillet metal");
    let post = bead(&list, "Post weld");
    assert_eq!((post["seam_count"].as_u64(), post["field"].as_bool(), post["note"].as_str()), (Some(1), Some(true), Some("GMAW")), "{post}");
    near(post["seam_mm"].as_f64().unwrap(), 200.0, "post outline only");
    assert_eq!(post["seams"][0].as_array().unwrap().len(), 5, "a closed square: {post}");
    let gusset = bead(&list, "Gusset weld");
    near(gusset["seam_mm"].as_f64().unwrap(), 206.0, "both sides and the far end");
    assert_eq!(gusset["welds"].as_u64(), Some(4), "{gusset}");
    near(gusset["length_mm"].as_f64().unwrap(), 120.0, "four 30 mm welds");
    // Picked by a point: only that seam (a gusset's both sides are one seam round it).
    run(&mut s, "weld.bead", json!({"bodies": ["Gusset", "Leg"], "size": 3, "kind": "groove", "at": [[25, 60, 3]], "name": "Picked"}));
    let picked = bead(&run(&mut s, "weld.beads", json!({"bodies": ["Leg"]})), "Picked").clone();
    assert_eq!((picked["kind"].as_str(), picked["volume_mm3"].as_f64()), (Some("groove"), None), "{picked}");
    near(picked["seam_mm"].as_f64().unwrap(), 206.0, "picked");
    // The weld table: one row per bead here, then the CSV's table.
    let table = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!((table["welds"].as_array().unwrap().len(), table["weld_count"].as_u64()), (4, Some(4)), "{table}");
    let csv = run(&mut s, "weld.export_cut_list", json!({}))["text"].as_str().unwrap().to_string();
    assert!(
        csv.contains("\r\nweld,qty,kind,size_mm,length_mm,welds,intermittent_length_mm,pitch_mm,field,volume_mm3,mass_kg,beads,bodies,note\r\n"),
        "{csv}"
    );
    assert!(csv.contains(",\"Post weld\",\"Post+Rail\",\"GMAW\"\r\n"), "{csv}");
    // Edit, redefine (the dialog's edit) and reopen.
    run(&mut s, "timeline.edit", json!({"feature": "Gusset weld", "set": {"pitch": "100 mm"}}));
    assert_eq!(bead(&run(&mut s, "weld.beads", json!({})), "Gusset weld")["welds"].as_u64(), Some(2));
    run(&mut s, "timeline.redefine", json!({"feature": "Post weld", "command": "weld.bead", "params": {"bodies": ["Post", "Rail"], "size": 6}}));
    let post = bead(&run(&mut s, "weld.beads", json!({})), "Post weld").clone();
    assert_eq!((post["size_mm"].as_f64(), post["field"].as_bool()), (Some(6.0), Some(false)), "{post}");
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    near(
        bead(&run(&mut reopened, "weld.beads", json!({})), "Mitre weld")["seam_mm"].as_f64().unwrap(),
        100.0 + 100.0 * 2f64.sqrt() - 6.0,
        "reopened",
    );
}

/// A gap is how far apart the bodies may stand, not how far they must: a gap wider than the
/// rail's 3 mm wall still finds the post's outline where the two touch, not the rail's bore.
#[test]
fn weld_bead_gaps_reach_past_thin_walls() {
    let mut s = welded();
    run(&mut s, "weld.bead", json!({"bodies": ["Post", "Rail"], "size": 4, "gap": 6, "name": "Gapped"}));
    let gapped = bead(&run(&mut s, "weld.beads", json!({})), "Gapped").clone();
    assert_eq!(gapped["seam_count"].as_u64(), Some(1), "{gapped}");
    near(gapped["seam_mm"].as_f64().unwrap(), 200.0, "post outline");
}

/// Identical beads share a row of the weld table.
#[test]
fn identical_weld_beads_share_a_row() {
    let mut s = welded();
    run(&mut s, "weld.bead", json!({"bodies": ["Gusset", "Rail"], "size": 3}));
    run(&mut s, "weld.bead", json!({"bodies": ["Gusset", "Leg"], "size": 3}));
    run(&mut s, "weld.bead", json!({"bodies": ["Post", "Rail"], "size": 3}));
    let welds = run(&mut s, "weld.cut_list", json!({}))["welds"].as_array().unwrap().clone();
    assert_eq!(welds.len(), 2, "{welds:?}");
    assert_eq!((welds[0]["weld"].as_u64(), welds[0]["qty"].as_u64()), (Some(1), Some(2)), "{welds:?}");
    assert_eq!(welds[0]["bodies"], json!([["Gusset", "Rail"], ["Gusset", "Leg"]]));
}

#[test]
fn wrong_weld_beads_are_refused() {
    let mut s = welded();
    for (p, want) in [
        (json!({"bodies": ["Post", "Leg"], "size": 5}), "don't meet"),
        (json!({"bodies": ["Rail", "Rail"], "size": 5}), "two different"),
        (json!({"bodies": ["Rail"], "size": 5}), "two bodies"),
        (json!({"bodies": ["Rail", "Nothing"], "size": 5}), "no body"),
        (json!({"bodies": ["Rail", "Leg"]}), "size"),
        (json!({"bodies": ["Rail", "Leg"], "size": -1}), "size"),
        (json!({"bodies": ["Rail", "Leg"], "size": 5, "kind": "spot"}), "fillet or groove"),
        (json!({"bodies": ["Rail", "Leg"], "size": 5, "length": 30}), "both"),
        (json!({"bodies": ["Rail", "Leg"], "size": 5, "length": 50, "pitch": 30}), "at least their length"),
        (json!({"bodies": ["Rail", "Leg"], "size": 5, "at": [[400, 300, 0]]}), "no seam"),
        (json!({"bodies": ["Rail", "Leg"], "size": 5, "at": [["x", 0, 0]]}), "points"),
        (json!({"bodies": ["Rail", "Leg"], "size": 5, "note": "x".repeat(300)}), "note"),
    ] {
        let e = exec(&mut s, "weld.bead", p.clone()).unwrap_err();
        assert!(e.contains(want), "{p}: {e}");
    }
    assert!(s.model.state().beads.is_empty());
}
