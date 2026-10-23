//! The weldment profile library through the commands: a profile drawn once, saved, found by
//! search and used by Structural Members without its sketch; each member keeps a copy, so the
//! design builds without the library.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::Session;

const AREA: f64 = 564.0;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// A library file of this test's own (tests run in parallel), not there yet.
fn library(tag: &str) -> String {
    let p = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-weld-library-{tag}-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p.to_string_lossy().into()
}

/// The tube profile `Tube` (50 × 50 × 3, sharp corners) on YZ, away from the origin so saving
/// centres it, and a skeleton `Frame` on XY: a 950 × 550 rectangle (lines `l1`…`l4`).
fn setup() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Tube"}));
    run(&mut s, "sketch.line", json!({"points": [[75, 75], [125, 75], [125, 125], [75, 125]], "closed": true}));
    run(&mut s, "sketch.line", json!({"points": [[78, 78], [122, 78], [122, 122], [78, 122]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Frame"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [950, 0], [950, 550], [0, 550]], "closed": true, "ids": ["l1", "l2", "l3", "l4"]}));
    run(&mut s, "sketch.finish", json!({}));
    s
}

fn save(s: &mut Session, lib: &str, name: &str) -> Result<Value, String> {
    s.execute("weld.profile_save", &json!({"name": name, "sketch": "Tube", "profiles": [{"point": [123.5, 100]}], "library": lib}))
        .map_err(|e| e.to_string())
}

fn measure(s: &Session, body: &str) -> solidcraft_kernel::BodyMeasure {
    solidcraft_kernel::measure(&s.world_state().body(body).unwrap_or_else(|| panic!("no body {body}")).body).unwrap()
}

fn near(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= want.abs() * 1e-6 + 1e-6, "{what}: {got} ≠ {want}");
}

/// Saved, listed with its size, found by any-case words of its name, and deleted.
#[test]
fn profiles_are_saved_found_and_deleted() {
    let lib = library("list");
    let mut s = setup();
    let empty = run(&mut s, "weld.profiles", json!({"library": lib}));
    assert_eq!(empty["total"], 0);
    let saved = save(&mut s, &lib, "SHS 50x50x3").unwrap();
    near(saved["profile"]["width_mm"].as_f64().unwrap(), 50.0, "width");
    near(saved["profile"]["area_mm2"].as_f64().unwrap(), AREA, "area");
    assert_eq!(saved["profile"]["holes"], 1);
    save(&mut s, &lib, "RHS tube").unwrap();
    let found = run(&mut s, "weld.profiles", json!({"library": lib, "query": "shs 50"}));
    let names: Vec<&str> = found["profiles"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["SHS 50x50x3"]);
    assert_eq!(found["total"], 2);
    assert_eq!(run(&mut s, "weld.profiles", json!({"library": lib, "query": "tube"}))["profiles"].as_array().unwrap().len(), 1);
    // Saving has not changed the design.
    assert_eq!(s.doc.features.len(), 2);
    run(&mut s, "weld.profile_delete", json!({"library": lib, "name": "shs 50X50X3"}));
    assert_eq!(run(&mut s, "weld.profiles", json!({"library": lib}))["total"], 1);
    let _ = std::fs::remove_file(&lib);
}

/// A name already there is refused unless replacing; an empty name and a sketch with two
/// separate regions are refused, and nothing is written.
#[test]
fn saving_refuses_duplicates_and_bad_profiles() {
    let lib = library("refuse");
    let mut s = setup();
    save(&mut s, &lib, "SHS").unwrap();
    assert!(save(&mut s, &lib, " shs ").unwrap_err().contains("already has"));
    s.execute("weld.profile_save", &json!({"name": "SHS", "sketch": "Tube", "profiles": [{"point": [123.5, 100]}], "library": lib, "replace": true}))
        .unwrap();
    assert_eq!(run(&mut s, "weld.profiles", json!({"library": lib}))["total"], 1);
    assert!(save(&mut s, &lib, "  ").is_err());
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Two"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [5, 5]}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [10, 0], "p1": [15, 5]}));
    run(&mut s, "sketch.finish", json!({}));
    assert!(s.execute("weld.profile_save", &json!({"name": "Two", "sketch": "Two", "library": lib})).is_err());
    assert_eq!(run(&mut s, "weld.profiles", json!({"library": lib}))["total"], 1);
    let _ = std::fs::remove_file(&lib);
}

/// A member from the library is the same solid as one from the sketch: the mitred rectangle,
/// each member the profile times its centre line, named in the cut list after the library
/// profile.
#[test]
fn a_library_member_matches_the_sketch_member() {
    let lib = library("member");
    let mut s = setup();
    save(&mut s, &lib, "SHS 50x50x3").unwrap();
    run(
        &mut s,
        "weld.member",
        json!({"stock": "SHS 50x50x3", "library": lib, "path_sketch": "Frame", "path": ["l1", "l2", "l3", "l4"], "body_name": "Rail"}),
    );
    for (name, length) in ["Rail", "Rail (2)", "Rail (3)", "Rail (4)"].iter().zip([950.0, 550.0, 950.0, 550.0]) {
        near(measure(&s, name).volume, AREA * length, name);
    }
    let lib_box = measure(&s, "Rail").bbox;
    run(
        &mut s,
        "weld.member",
        json!({"sketch": "Tube", "profiles": [{"point": [123.5, 100]}], "path_sketch": "Frame", "path": ["l1"], "body_name": "Check", "join_earlier": false}),
    );
    let sketch_box = measure(&s, "Check").bbox;
    near(lib_box.min.y, sketch_box.min.y, "same place");
    near(lib_box.max.z, sketch_box.max.z, "same height");
    let list = run(&mut s, "weld.cut_list", json!({"bodies": ["Rail", "Rail (2)"]}));
    assert_eq!(list["rows"][0]["description"], "SHS 50x50x3");
    let _ = std::fs::remove_file(&lib);
}

/// The design keeps its copy: with the library gone (and the profile sketch deleted) it reopens
/// and builds the same members, and editing the member keeps the copy.
#[test]
fn the_design_builds_without_the_library() {
    let lib = library("reopen");
    let mut s = setup();
    save(&mut s, &lib, "SHS").unwrap();
    run(&mut s, "weld.member", json!({"stock": "SHS", "library": lib, "path_sketch": "Frame", "path": ["l1", "l2"], "body_name": "Rail"}));
    std::fs::remove_file(&lib).unwrap();
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    near(measure(&reopened, "Rail").volume, measure(&s, "Rail").volume, "reopened");
    run(&mut reopened, "timeline.edit", json!({"feature": "StructuralMember1", "set": {"gap": "1 mm"}}));
    assert!(reopened.world_state().body("Rail (2)").is_some());
    // A name not in the library is refused, never another profile.
    assert!(s.execute("weld.member", &json!({"stock": "SHS", "library": lib, "path_sketch": "Frame", "path": ["l3"]})).is_err());
}

/// Both `sketch` and `stock`, or a hand-made stock that isn't a closed bounded region, are
/// refused before anything changes; a design file holding such a stock fails the feature, not
/// the app.
#[test]
fn bad_stock_is_refused() {
    let mut s = setup();
    let square = json!({"outer": {"segs": [
        {"type": "line", "a": [0, 0], "b": [10, 0]}, {"type": "line", "a": [10, 0], "b": [10, 10]},
        {"type": "line", "a": [10, 10], "b": [0, 10]}, {"type": "line", "a": [0, 10], "b": [0, 0]}]}, "holes": []});
    let base = json!({"path_sketch": "Frame", "path": ["l1"]});
    let with = |extra: Value| {
        let mut p = base.clone();
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    let features = s.doc.features.len();
    assert!(s.execute("weld.member", &with(json!({"stock": {"name": "Bar", "region": square}, "sketch": "Tube"}))).is_err());
    let mut open = square.clone();
    open["outer"]["segs"].as_array_mut().unwrap().pop();
    assert!(s.execute("weld.member", &with(json!({"stock": {"name": "Bar", "region": open}}))).is_err());
    let mut huge = square.clone();
    huge["outer"]["segs"][0]["b"] = json!([1e300, 0]);
    assert!(s.execute("weld.member", &with(json!({"stock": {"name": "Bar", "region": huge}}))).is_err());
    assert!(s.execute("weld.member", &with(json!({"stock": {"name": "", "region": square}}))).is_err());
    assert_eq!(s.doc.features.len(), features);
    // A good inline copy works: a 10 × 10 bar.
    run(&mut s, "weld.member", with(json!({"stock": {"name": "Bar", "region": square}, "body_name": "Bar"})));
    near(measure(&s, "Bar").volume, 100.0 * 950.0, "bar");
    // The same feature edited by hand in the design file to an open outline: an error on it.
    let mut v: Value = serde_json::from_str(&s.doc.to_json()).unwrap();
    let f = v["features"].as_array_mut().unwrap().iter_mut().find(|f| f["stock"].is_object()).unwrap();
    f["stock"]["region"]["outer"]["segs"].as_array_mut().unwrap().pop();
    let reopened = Session::new(solidcraft_doc::Document::from_json(&v.to_string()).unwrap());
    assert!(reopened.world_state().body("Bar").is_none());
}

/// Profiles group by type (their name up to the first number) and sort by the numbers in their
/// size, fractions included; `type` lists one type.
#[test]
fn profiles_group_by_type_and_sort_by_size() {
    let lib = library("types");
    let mut s = setup();
    for name in ["IPE 100", "HSS 2x2x3/16", "Tube", "IPE 80", "HSS 2x2x1/8", "HSS 1-1/2x1-1/2x1/8"] {
        save(&mut s, &lib, name).unwrap();
    }
    let all = run(&mut s, "weld.profiles", json!({"library": lib}));
    let types: Vec<(String, u64)> =
        all["types"].as_array().unwrap().iter().map(|t| (t["type"].as_str().unwrap().to_string(), t["count"].as_u64().unwrap())).collect();
    assert_eq!(types, [("HSS".to_string(), 3), ("IPE".to_string(), 2), ("Other".to_string(), 1)]);
    let hss = run(&mut s, "weld.profiles", json!({"library": lib, "type": "hss"}));
    let sizes: Vec<&str> = hss["profiles"].as_array().unwrap().iter().map(|p| p["size"].as_str().unwrap()).collect();
    assert_eq!(sizes, ["1-1/2x1-1/2x1/8", "2x2x1/8", "2x2x3/16"]);
    let ipe = run(&mut s, "weld.profiles", json!({"library": lib, "type": "IPE"}));
    let names: Vec<&str> = ipe["profiles"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["IPE 80", "IPE 100"]);
    let _ = std::fs::remove_file(&lib);
}

/// A library profile's stock lengths are copied into the members made from it and count the
/// bars to buy; the design's own lengths are used over them, and changing the library's later
/// changes no member already made.
#[test]
fn library_stock_lengths_travel_with_the_profile() {
    let lib = library("lengths");
    let mut s = setup();
    let saved = s
        .execute(
            "weld.profile_save",
            &json!({"name": "SHS", "sketch": "Tube", "profiles": [{"point": [123.5, 100]}], "lengths": [6000, 3000], "library": lib}),
        )
        .unwrap();
    assert_eq!(saved["profile"]["lengths_mm"], json!([6000.0, 3000.0]));
    run(
        &mut s,
        "weld.member",
        json!({"stock": "SHS", "library": lib, "path_sketch": "Frame", "path": ["l1", "l2", "l3", "l4"], "body_name": "Rail"}),
    );
    let stock = |s: &mut Session| run(s, "weld.cut_list", json!({"kerf": 0}))["stock"][0].clone();
    let x = stock(&mut s);
    assert_eq!(x["lengths_from"], "library");
    assert_eq!(x["buy"], json!([{"length_mm": 6000.0, "qty": 1}]));
    let changed = run(&mut s, "weld.profile_lengths", json!({"name": "shs", "lengths": [12000], "library": lib}));
    assert_eq!(changed["profile"]["lengths_mm"], json!([12000.0]));
    assert_eq!(stock(&mut s)["stock_lengths_mm"], json!([6000.0, 3000.0]));
    run(&mut s, "weld.stock_lengths", json!({"profile": "SHS", "lengths": [1500]}));
    let x = stock(&mut s);
    assert_eq!(x["lengths_from"], "design");
    assert_eq!(x["buy"], json!([{"length_mm": 1500.0, "qty": 3}]));
    assert!(s.execute("weld.profile_lengths", &json!({"name": "SHS", "lengths": [-5], "library": lib})).is_err());
    assert!(s.execute("weld.profile_lengths", &json!({"name": "nope", "lengths": [5], "library": lib})).is_err());
    let _ = std::fs::remove_file(&lib);
}
