//! Cut list properties: a library profile's description and free properties carried to its
//! members' rows, a body's own set with `weld.cut_properties` over them, and bodies left out of
//! the cut list.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// A library file of this test's own (tests run in parallel), not there yet.
fn library(tag: &str) -> String {
    let p = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-weld-cut-props-{tag}-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p.to_string_lossy().into()
}

/// A 50 × 50 × 3 tube profile `Tube` on YZ and a mitred 950 × 550 frame of it, members `Rail`
/// … `Rail (4)`: two rows of two.
fn frame() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ", "name": "Tube"}));
    run(&mut s, "sketch.line", json!({"points": [[-25, -25], [25, -25], [25, 25], [-25, 25]], "closed": true}));
    run(&mut s, "sketch.line", json!({"points": [[-22, -22], [22, -22], [22, 22], [-22, 22]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Frame"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [950, 0], [950, 550], [0, 550]], "closed": true, "ids": ["l1", "l2", "l3", "l4"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "weld.member",
        json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Frame", "path": ["l1", "l2", "l3", "l4"], "body_name": "Rail"}),
    );
    s
}

/// Each row's (qty, description, members).
fn rows(list: &Value) -> Vec<(u64, String, Vec<String>)> {
    list["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let members = r["members"].as_array().unwrap().iter().map(|m| m.as_str().unwrap().to_string()).collect();
            (r["qty"].as_u64().unwrap(), r["description"].as_str().unwrap().to_string(), members)
        })
        .collect()
}

#[test]
fn a_library_profile_gives_its_members_description_and_properties() {
    let lib = library("library");
    let mut s = frame();
    let saved = run(
        &mut s,
        "weld.profile_save",
        json!({"name": "SHS 50x50x3", "sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "library": lib,
            "description": "Square tube 50 × 50 × 3", "properties": {"Standard": "EN 10219", "Grade": "S355", "Empty": ""}}),
    );
    assert_eq!(saved["profile"]["description"], "Square tube 50 × 50 × 3");
    assert_eq!(saved["profile"]["properties"], json!({"Grade": "S355", "Standard": "EN 10219"}));
    run(
        &mut s,
        "weld.member",
        json!({"stock": "SHS 50x50x3", "library": lib, "path_sketch": "Frame", "path": ["l1"], "body_name": "Stock", "join_earlier": false}),
    );
    std::fs::remove_file(&lib).unwrap();
    let list = run(&mut s, "weld.cut_list", json!({"bodies": ["Stock"]}));
    let row = &list["rows"][0];
    assert_eq!((row["description"].as_str(), row["profile"].as_str()), (Some("Square tube 50 × 50 × 3"), Some("SHS 50x50x3")));
    assert_eq!(row["properties"], json!({"Grade": "S355", "Standard": "EN 10219"}));
    assert_eq!(list["stock"][0]["description"], "SHS 50x50x3", "stock is totalled by profile");
    // The member keeps the copy: the design reopens with them, the library gone.
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    let mut reopened = reopened;
    let again = run(&mut reopened, "weld.cut_list", json!({"bodies": ["Stock"]}));
    assert_eq!(again["rows"][0]["properties"]["Standard"], "EN 10219");
    // A body's own description and properties go over the library's.
    run(
        &mut s,
        "weld.cut_properties",
        json!({"bodies": ["Stock"], "description": "Top rail", "properties": {"Grade": "S235", "Finish": "galvanised"}}),
    );
    let row = run(&mut s, "weld.cut_list", json!({"bodies": ["Stock"]}))["rows"][0].clone();
    assert_eq!(row["description"], "Top rail");
    assert_eq!(row["properties"], json!({"Finish": "galvanised", "Grade": "S235", "Standard": "EN 10219"}));
    // Clearing them shows the library's again.
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Stock"], "description": "", "properties": {"Grade": null}}));
    let row = run(&mut s, "weld.cut_list", json!({"bodies": ["Stock"]}))["rows"][0].clone();
    assert_eq!(row["description"], "Square tube 50 × 50 × 3");
    assert_eq!(row["properties"]["Grade"], "S355");
}

#[test]
fn bodies_with_other_properties_are_listed_apart() {
    let mut s = frame();
    let before = rows(&run(&mut s, "weld.cut_list", json!({})));
    assert_eq!(before.iter().map(|r| r.0).collect::<Vec<_>>(), [2, 2]);
    assert_eq!(before[0].1, "Tube", "no description: the profile sketch's name");
    let r = run(&mut s, "weld.cut_properties", json!({"bodies": ["Rail"], "properties": {"Mark": "A"}}));
    assert_eq!(r["changed"], json!(["properties.Mark"]));
    let split = rows(&run(&mut s, "weld.cut_list", json!({})));
    assert_eq!(split.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 2, 1], "{split:?}");
    // Giving its twin the same property puts them back in one row.
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Rail (3)"], "properties": {"Mark": "A"}}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(rows(&list).iter().map(|r| r.0).collect::<Vec<_>>(), [2, 2]);
    assert_eq!(list["rows"][0]["properties"], json!({"Mark": "A"}));
    // The export has a column for it.
    let csv = run(&mut s, "weld.export_cut_list", json!({}))["text"].as_str().unwrap().to_string();
    let lines: Vec<&str> = csv.lines().collect();
    assert!(lines[0].ends_with(",mirrored,profile,\"Mark\""), "{csv}");
    assert!(lines[1].ends_with(",\"Tube\",\"A\""), "{csv}");
    assert!(lines[2].ends_with(",\"Tube\",\"\""), "{csv}");
    // Setting the same again changes nothing (no undo step).
    let r = run(&mut s, "weld.cut_properties", json!({"bodies": ["Rail", "Rail (3)"], "properties": {"Mark": "A"}}));
    assert_eq!(r["changed"], json!([]));
}

#[test]
fn excluded_bodies_are_left_out_until_included() {
    let mut s = frame();
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Rail (2)"], "exclude": true}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(list["members"], 3);
    assert_eq!(list["excluded"], json!(["Rail (2)"]));
    assert!(rows(&list).iter().all(|r| !r.2.contains(&"Rail (2)".to_string())));
    assert_eq!(list["stock"][0]["members"], 3);
    // One undo step brings it back, as does including it again.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(run(&mut s, "weld.cut_list", json!({}))["members"], 4);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(run(&mut s, "weld.cut_list", json!({}))["members"], 3);
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Rail (2)"], "exclude": false}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!((list["members"].as_u64(), list["excluded"].as_array().map(Vec::len)), (Some(4), Some(0)));
    assert!(s.doc.cut_list.is_empty(), "nothing set is nothing saved");
}

#[test]
fn properties_follow_a_renamed_body_and_are_saved() {
    let mut s = frame();
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Rail"], "description": "Bottom", "exclude": true}));
    run(&mut s, "body.rename", json!({"body": "Rail", "name": "Bottom rail"}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(list["excluded"], json!(["Bottom rail"]));
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    run(&mut reopened, "weld.cut_properties", json!({"bodies": ["Bottom rail"], "exclude": false}));
    let list = run(&mut reopened, "weld.cut_list", json!({}));
    assert!(rows(&list).iter().any(|r| r.1 == "Bottom" && r.2 == ["Bottom rail"]), "{list}");
}

#[test]
fn bad_cut_properties_are_refused() {
    let mut s = frame();
    for p in [
        json!({"bodies": [], "description": "x"}),
        json!({"bodies": ["Nope"], "description": "x"}),
        json!({"bodies": ["Rail"]}),
        json!({"bodies": ["Rail"], "properties": {"": "x"}}),
        json!({"bodies": ["Rail"], "properties": {"Mark": ["x"]}}),
        json!({"bodies": ["Rail"], "properties": "Mark"}),
        json!({"bodies": ["Rail"], "description": "x".repeat(5000)}),
        json!({"bodies": ["Rail"], "exclude": "yes"}),
        json!({"bodies": ["Rail", "Nope"], "exclude": true}),
    ] {
        assert!(s.execute("weld.cut_properties", &p).is_err(), "{p}");
    }
    assert!(s.doc.cut_list.is_empty(), "a refused call changes nothing");
}

/// Bodies that are neither members nor plates are listed after them by their smallest bounding
/// box: a plate turned 30° matches its twin, a thicker one is a row of its own; they take
/// properties and can be left out like members.
#[test]
fn other_bodies_are_listed_by_their_bounding_box() {
    let mut s = frame();
    run(&mut s, "solid.box", json!({"length": 200, "width": 120, "height": 8, "corner": [0, 600, 0], "body_name": "Base"}));
    run(&mut s, "solid.box", json!({"length": 200, "width": 120, "height": 8, "corner": [300, 600, 0], "body_name": "Base 2"}));
    run(&mut s, "solid.move", json!({"bodies": ["Base 2"], "axis": [0, 0, 1], "angle": "30 deg"}));
    run(&mut s, "solid.box", json!({"length": 200, "width": 120, "height": 10, "corner": [600, 600, 0], "body_name": "Thick"}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(list["members"], 4);
    let bodies = list["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 2, "{list}");
    assert_eq!((bodies[0]["item"].as_u64(), bodies[0]["qty"].as_u64(), bodies[0]["kind"].as_str()), (Some(3), Some(2), Some("solid")));
    assert_eq!(bodies[0]["bodies"], json!(["Base", "Base 2"]));
    let size = |r: &Value| ["length_mm", "width_mm", "thickness_mm"].map(|k| r[k].as_f64().unwrap());
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
    assert!(close(size(&bodies[0]), [200.0, 120.0, 8.0]), "{list}");
    assert!(close(size(&bodies[1]), [200.0, 120.0, 10.0]), "{list}");
    assert!((bodies[0]["volume_mm3"].as_f64().unwrap() - 192_000.0).abs() < 1.0);
    assert_eq!(list["body_count"], 3);
    // Properties and exclusion work on them too; other_bodies: false leaves them all out.
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Thick"], "description": "Foot plate", "exclude": false}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(list["bodies"][1]["description"], "Foot plate");
    run(&mut s, "weld.cut_properties", json!({"bodies": ["Base 2"], "exclude": true}));
    let list = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!((list["body_count"].as_u64(), list["excluded"].clone()), (Some(2), json!(["Base 2"])));
    let without = run(&mut s, "weld.cut_list", json!({"other_bodies": false}));
    assert_eq!((without["bodies"].as_array().map(Vec::len), without["body_count"].as_u64()), (Some(0), Some(0)));
    // The export's third table.
    let csv = run(&mut s, "weld.export_cut_list", json!({}))["text"].as_str().unwrap().to_string();
    assert!(csv.contains("\r\n\r\nitem,qty,body,length_mm,width_mm,thickness_mm,volume_mm3,mass_kg,material,bodies,description\r\n"), "{csv}");
    assert!(csv.contains("\r\n3,1,solid,200.000,120.000,8.000,192000.00,"), "{csv}");
    assert!(s.execute("weld.cut_list", &json!({"other_bodies": "yes"})).is_err());
    assert!(s.execute("weld.cut_list", &json!({"bodies": ["Nope"]})).is_err());
}

#[test]
fn a_sub_weldment_is_a_cut_list_section_of_its_own() {
    let mut s = frame();
    let made = run(&mut s, "weld.sub_weldment", json!({"bodies": ["Rail (3)", "Rail (4)"]}));
    assert_eq!(made["name"], "Sub-weldment 1");
    let id = made["sub_weldment"].clone();
    // Identical members in different sections are different rows; the weldment's own come first.
    let list = run(&mut s, "weld.cut_list", json!({}));
    let names = |l: &Value| rows(l).into_iter().map(|r| r.2).collect::<Vec<_>>();
    assert_eq!(names(&list), [vec!["Rail"], vec!["Rail (2)"], vec!["Rail (3)"], vec!["Rail (4)"]]);
    let sections: Vec<&Value> = list["rows"].as_array().unwrap().iter().map(|r| &r["sub_weldment"]).collect();
    assert_eq!(sections, [&Value::Null, &Value::Null, &json!("Sub-weldment 1"), &json!("Sub-weldment 1")]);
    assert_eq!(list["sub_weldments"], json!([{"id": id, "name": "Sub-weldment 1", "bodies": ["Rail (3)", "Rail (4)"], "items": [3, 4]}]));
    // One section alone, by name or id.
    let alone = run(&mut s, "weld.cut_list", json!({"sub_weldment": "Sub-weldment 1"}));
    assert_eq!(names(&alone), [vec!["Rail (3)"], vec!["Rail (4)"]]);
    assert_eq!(names(&run(&mut s, "weld.cut_list", json!({"sub_weldment": id}))), names(&alone));
    assert!(s.execute("weld.cut_list", &json!({"sub_weldment": "Nope"})).is_err());
    // Names are unique; a rename and a renamed body are followed.
    run(&mut s, "weld.sub_weldment", json!({"bodies": ["Rail"], "name": "Base"}));
    assert!(s.execute("weld.rename_sub_weldment", &json!({"sub_weldment": "Base", "name": "Sub-weldment 1"})).is_err());
    run(&mut s, "weld.rename_sub_weldment", json!({"sub_weldment": id, "name": "Top"}));
    run(&mut s, "body.rename", json!({"body": "Rail (4)", "name": "Top rail"}));
    assert_eq!(s.doc.sub_weldments.iter().find(|w| w.name == "Top").unwrap().bodies, ["Rail (3)", "Top rail"]);
    // A body moved into another sub-weldment leaves its old one, which goes once empty.
    run(&mut s, "weld.sub_weldment", json!({"bodies": ["Rail"], "sub_weldment": "Top"}));
    assert_eq!(s.doc.sub_weldments.len(), 1, "Base had only Rail");
    // The CSV has a sub_weldment column while there are sub-weldments.
    let csv = run(&mut s, "weld.export_cut_list", json!({}));
    let text = csv["text"].as_str().unwrap();
    assert!(text.lines().next().unwrap().ends_with(",profile,sub_weldment"), "{text}");
    assert!(text.contains(",\"Top\""), "{text}");
    // Dissolved one body at a time, then all: the rows merge again.
    let left = run(&mut s, "weld.dissolve_sub_weldment", json!({"sub_weldment": "Top", "bodies": ["Rail"]}));
    assert_eq!(left["bodies"], json!(["Rail"]));
    assert!(s.execute("weld.dissolve_sub_weldment", &json!({"sub_weldment": "Top", "bodies": ["Rail (2)"]})).is_err());
    run(&mut s, "weld.dissolve_sub_weldment", json!({"sub_weldment": "Top"}));
    assert!(s.doc.sub_weldments.is_empty());
    let merged = run(&mut s, "weld.cut_list", json!({}));
    assert_eq!(rows(&merged).len(), 2);
    assert!(!run(&mut s, "weld.export_cut_list", json!({}))["text"].as_str().unwrap().contains("sub_weldment"));
    // Saved with the design.
    run(&mut s, "edit.undo", json!({}));
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert_eq!(reopened.doc.sub_weldments, s.doc.sub_weldments);
    assert_eq!(reopened.doc.sub_weldments.len(), 1);
}
