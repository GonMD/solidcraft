//! The Structural Member dialog's skeleton groups: Add group opens a group's input, each group's
//! lines are read from the sketch they were picked in (line ids repeat across sketches), and
//! editing keeps the groups and moves joints with their lines; the Corners rows set joints. The
//! located point is picked in the view or on the profile's outline. The Cut List edits an item's
//! description and properties.
use crate::dialogs::Kind;
use crate::scenario::Harness;
use crate::viewport::Hit;
use serde_json::json;
use solidcraft_engine::Sel;
use solidcraft_engine::doc::{FeatureKind, SketchId, WeldCorner};

/// A tube profile `Tube` and three skeleton sketches: `Frame` (`l1` along X), `Side` (also `l1`,
/// up Y from Frame's end) and `Top` (`t`, up Y from the origin). Their ids, in that order. The
/// window is tall enough for the whole Structural Member dialog, so its lower rows are clicked
/// without scrolling.
fn setup(h: &mut Harness) -> [u64; 4] {
    h.call("ui.resize", json!({"width": 1600, "height": 1400}));
    for (cmd, p) in [
        ("sketch.create", json!({"plane": "YZ", "name": "Tube"})),
        ("sketch.line", json!({"points": [[-25, -25], [25, -25], [25, 25], [-25, 25]], "closed": true})),
        ("sketch.line", json!({"points": [[-22, -22], [22, -22], [22, 22], [-22, 22]], "closed": true})),
        ("sketch.finish", json!({})),
        ("sketch.create", json!({"plane": "XY", "name": "Frame"})),
        ("sketch.line", json!({"points": [[0, 0], [950, 0]], "ids": ["l1"]})),
        ("sketch.finish", json!({})),
        ("sketch.create", json!({"plane": "XY", "name": "Side"})),
        ("sketch.line", json!({"points": [[950, 0], [950, 550]], "ids": ["l1"]})),
        ("sketch.finish", json!({})),
        ("sketch.create", json!({"plane": "XY", "name": "Top"})),
        ("sketch.line", json!({"points": [[0, 0], [0, 550]], "ids": ["t"]})),
        ("sketch.finish", json!({})),
    ] {
        h.app.run(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    }
    ["Tube", "Frame", "Side", "Top"].map(|n| h.app.session.doc.find_feature(n).unwrap().id)
}

/// A line of `sketch` picked in the view into the dialog's active input.
fn pick(h: &mut Harness, sketch: u64, id: &str) {
    let hit = Hit::SketchCurve { sketch, id: id.into(), straight: true };
    crate::viewport::dialog_pick(&mut h.app, &hit, Sel::SketchCurve { id: id.into() });
}

#[test]
fn add_group_takes_lines_of_another_sketch() {
    let mut h = Harness::new();
    let [tube, frame, side, _] = setup(&mut h);
    h.step(&json!({"call": "ui.start", "params": {"command": "weld.member"}})).unwrap();
    h.frames(3);
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::Profile { sketch: tube, index: 0 }];
    h.app.dialog.as_mut().unwrap().active = 1;
    pick(&mut h, frame, "l1");
    h.frames(2);
    h.step(&json!({"widget": "Add group"})).unwrap();
    h.frames(2);
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!((d.inputs.len(), d.active), (3, 2), "the group's input is open for picking");
    assert!(h.widget("Group 2").is_some());
    pick(&mut h, side, "l1");
    let Kind::WeldMember { groups, .. } = &mut h.app.dialog.as_mut().unwrap().kind else { panic!("not a member dialog") };
    groups.rows[0].corner = 2;
    h.frames(2);
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    let f = h.app.session.doc.features.last().unwrap();
    let FeatureKind::WeldMember { path_sketch, path, groups, .. } = &f.kind else { panic!("{:?}", f.kind) };
    assert_eq!((*path_sketch, path.as_slice()), (SketchId(frame), &["l1".to_string()][..]));
    assert_eq!(groups.len(), 1);
    assert_eq!(
        (groups[0].path_sketch, groups[0].path.as_slice(), groups[0].corner),
        (SketchId(side), &["l1".to_string()][..], Some(WeldCorner::Butt))
    );
    assert!(h.app.session.model.results.iter().all(|r| r.error.is_none()));
}

#[test]
fn a_new_group_without_lines_keeps_the_preview() {
    let mut h = Harness::new();
    let [tube, frame, _, _] = setup(&mut h);
    h.step(&json!({"call": "ui.start", "params": {"command": "weld.member"}})).unwrap();
    h.frames(3);
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::Profile { sketch: tube, index: 0 }];
    h.app.dialog.as_mut().unwrap().active = 1;
    pick(&mut h, frame, "l1");
    h.frames(2);
    h.step(&json!({"widget": "Add group"})).unwrap();
    h.frames(2);
    let d = h.app.dialog.as_ref().unwrap();
    let cmds = crate::dialogs::apply_commands(&h.app, d).expect("the Skeleton alone still makes the member");
    assert!(cmds[0].1.get("groups").is_none(), "the empty group is left out: {:?}", cmds[0].1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.app.preview.busy || !h.app.preview.active {
        assert!(std::time::Instant::now() < deadline, "the preview did not come up");
        h.frames(1);
    }
    assert_eq!(h.app.preview.error, None);
}

#[test]
fn editing_keeps_groups_and_moves_joints_with_their_lines() {
    let mut h = Harness::new();
    let [_, _, side, top] = setup(&mut h);
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Frame", "path": ["l1"], "body_name": "Rail",
        "groups": [{"path_sketch": side, "path": ["l1"], "angle": "45 deg"}, {"path_sketch": top, "path": ["t"], "corner": "butt"}],
        "joints": [{"members": ["l1", "3:t"], "corner": "butt", "through": "3:t"}]});
    h.app.run("weld.member", p).unwrap();
    let id = h.app.session.doc.features.last().unwrap().id;
    h.step(&json!({"call": "ui.editFeature", "params": {"feature": id}})).unwrap();
    h.frames(2);
    let d = h.app.dialog.as_mut().unwrap();
    assert_eq!(d.inputs.iter().map(|i| i.label).collect::<Vec<_>>(), ["Profile", "Skeleton", "Group 2", "Group 3"]);
    let Kind::WeldMember { groups, .. } = &mut d.kind else { panic!("not a member dialog") };
    assert_eq!(groups.rows.iter().map(|r| (r.angle.as_str(), r.corner)).collect::<Vec<_>>(), [("45 deg", 0), ("", 2)]);
    // Group 2 removed, as its × does.
    groups.rows.remove(0);
    groups.sketches.remove(1);
    d.inputs.remove(2);
    h.frames(2);
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let f = h.app.session.doc.feature(id).unwrap();
    let FeatureKind::WeldMember { groups, joints, .. } = &f.kind else { panic!("{:?}", f.kind) };
    assert_eq!(groups.len(), 1);
    assert_eq!(serde_json::to_value(joints).unwrap(), json!([{"members": ["l1", "2:t"], "corner": "butt", "through": "2:t"}]));
    let lines: Vec<(String, String)> = h.app.session.world_state().welds.iter().map(|w| (w.line.clone(), w.body.clone())).collect();
    assert_eq!(lines, [("l1".to_string(), "Rail".to_string()), ("2:t".to_string(), "Rail (3)".to_string())]);
}

/// Editing a member fills in its located point and mirror (a group's own too, a point in the
/// profile's coordinates kept as it is) and OK saves them back; the quarter-turn buttons step a
/// plain angle round and add to an expression.
#[test]
fn editing_keeps_the_located_point_and_mirror() {
    use crate::dialogs_weld::turned;
    use solidcraft_engine::doc::{WeldLocate, WeldMirror};
    assert_eq!(turned("0 deg", 90.0), "90 deg");
    assert_eq!(turned("", -90.0), "270 deg");
    assert_eq!(turned("300 deg", 90.0), "30 deg");
    assert_eq!(turned("22.5°", 90.0), "112.5 deg");
    assert_eq!(turned("tilt", -90.0), "tilt - 90 deg");
    let mut h = Harness::new();
    let [_, _, side, _] = setup(&mut h);
    let p = json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Frame", "path": ["l1"], "body_name": "Rail",
        "locate": "top_left", "mirror": "up_down", "groups": [{"path_sketch": side, "path": ["l1"], "locate": {"point": [25, 0]}, "mirror": "none"}]});
    h.app.run("weld.member", p).unwrap();
    let id = h.app.session.doc.features.last().unwrap().id;
    h.step(&json!({"call": "ui.editFeature", "params": {"feature": id}})).unwrap();
    h.frames(2);
    let d = h.app.dialog.as_mut().unwrap();
    let Kind::WeldMember { place, groups, .. } = &mut d.kind else { panic!("not a member dialog") };
    assert_eq!((place.locate, place.point, place.mirror), (1, None, 2));
    let g = &groups.rows[0];
    assert_eq!((g.locate, g.point, g.mirror), (11, Some([25.0, 0.0]), 1));
    // Bottom right, mirrored left to right.
    place.locate = 9;
    place.mirror = 1;
    h.frames(2);
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let FeatureKind::WeldMember { locate, mirror, groups, .. } = &h.app.session.doc.feature(id).unwrap().kind else { panic!() };
    assert_eq!((*locate, *mirror), (WeldLocate::BottomRight, WeldMirror::LeftRight));
    assert_eq!((groups[0].locate, groups[0].mirror), (Some(WeldLocate::Point([25.0, 0.0])), Some(WeldMirror::None)));
}

/// The Corners rows list the corner a Skeleton line and a group's line make; one joined
/// otherwise (a butt, the group's member running through) is saved as a joint by its members'
/// keys, and editing reads it back by its lines.
#[test]
fn a_corner_joined_otherwise_is_saved_as_a_joint() {
    let mut h = Harness::new();
    let [tube, frame, side, _] = setup(&mut h);
    h.step(&json!({"call": "ui.start", "params": {"command": "weld.member"}})).unwrap();
    h.frames(3);
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::Profile { sketch: tube, index: 0 }];
    h.app.dialog.as_mut().unwrap().active = 1;
    pick(&mut h, frame, "l1");
    h.frames(2);
    h.step(&json!({"widget": "Add group"})).unwrap();
    h.frames(2);
    pick(&mut h, side, "l1");
    h.frames(2);
    let Kind::WeldMember { groups, .. } = &mut h.app.dialog.as_mut().unwrap().kind else { panic!("not a member dialog") };
    let corners = groups.corners.as_ref().and_then(|(_, f)| f.as_ref().ok()).cloned().expect("the corners are listed");
    assert_eq!(corners["corners"][0]["members"], json!(["l1", "2:l1"]), "{corners}");
    assert_eq!(corners["corners"][0]["corner"], "mitre");
    // "2:l1 runs through", as its row's list sets it.
    groups.joints.push(crate::dialogs_weld::LineJoint {
        lines: [(frame, "l1".into()), (side, "l1".into())],
        corner: WeldCorner::Butt,
        through: Some(1),
    });
    h.frames(2);
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    let id = h.app.session.doc.features.last().unwrap().id;
    let f = h.app.session.doc.feature(id).unwrap();
    let FeatureKind::WeldMember { joints, .. } = &f.kind else { panic!("{:?}", f.kind) };
    assert_eq!(serde_json::to_value(joints).unwrap(), json!([{"members": ["l1", "2:l1"], "corner": "butt", "through": "2:l1"}]));
    h.step(&json!({"call": "ui.editFeature", "params": {"feature": id}})).unwrap();
    h.frames(2);
    let Kind::WeldMember { groups, .. } = &h.app.dialog.as_ref().unwrap().kind else { panic!("not a member dialog") };
    assert_eq!(groups.joints.len(), 1);
    assert_eq!((groups.joints[0].corner, groups.joints[0].through), (WeldCorner::Butt, Some(1)));
    let corners = groups.corners.as_ref().and_then(|(_, f)| f.as_ref().ok()).cloned().expect("the corners are listed");
    assert_eq!(corners["corners"][0]["through"], "2:l1");
    assert_eq!(corners["corners"][0]["own"], true);
}

/// The tube's outline offers its centroid, its box points (the middle is the centroid) and the
/// hole's four corners (the outside's are the box corners).
#[test]
fn the_locate_outline_offers_box_points_and_corners() {
    use crate::dialogs_weld::locate_marks;
    use solidcraft_engine::geom::{Loop2, Region2, Vec2};
    let square = |r: f64| Loop2::polygon(&[Vec2::new(-r, -r), Vec2::new(r, -r), Vec2::new(r, r), Vec2::new(-r, r)]);
    let tube = Region2 { outer: square(25.0), holes: vec![square(22.0).reversed()] };
    let marks = locate_marks(&tube);
    let named: Vec<usize> = marks.iter().filter_map(|m| m.locate).collect();
    assert_eq!(named, vec![0, 1, 2, 3, 4, 6, 7, 8, 9], "Middle is the centroid");
    let corners: Vec<Vec2> = marks.iter().filter(|m| m.locate.is_none()).map(|m| m.at).collect();
    assert_eq!(corners.len(), 4, "{corners:?}");
    assert!(corners.iter().all(|c| (c.x.abs() - 22.0).abs() < 1e-9 && (c.y.abs() - 22.0).abs() < 1e-9));
    // A round bar has no corners.
    let bar = Region2 { outer: Loop2::circle(Vec2::new(0.0, 0.0), 10.0), holes: Vec::new() };
    assert!(locate_marks(&bar).iter().all(|m| m.locate.is_some()));
}

/// Pick in view takes a point of the profile's sketch (only that sketch's) as the located point;
/// the outline under Locate sets a box point by name and a corner as a point; OK saves it.
#[test]
fn the_located_point_is_picked_in_the_view_or_on_the_outline() {
    use solidcraft_engine::doc::WeldLocate;
    let mut h = Harness::new();
    let [tube, frame, _, _] = setup(&mut h);
    h.step(&json!({"call": "ui.start", "params": {"command": "weld.member"}})).unwrap();
    h.frames(3);
    h.app.dialog.as_mut().unwrap().inputs[0].items = vec![Sel::Profile { sketch: tube, index: 0 }];
    h.app.dialog.as_mut().unwrap().active = 1;
    pick(&mut h, frame, "l1");
    h.frames(2);
    h.step(&json!({"widget": "Pick in view"})).unwrap();
    h.frames(2);
    let place = |h: &Harness| match &h.app.dialog.as_ref().unwrap().kind {
        Kind::WeldMember { place, .. } => place.clone(),
        _ => panic!("not a member dialog"),
    };
    assert!(place(&h).picking);
    let st = h.app.session.world_state();
    let corner = st.sketch(tube).unwrap().sketch.points.iter().find(|p| (p.pos.x - 25.0).abs() < 1e-9 && (p.pos.y - 25.0).abs() < 1e-9).unwrap();
    let (id, at) = (corner.id.clone(), corner.pos);
    let line = st.sketch(frame).unwrap().sketch.points.last().unwrap().clone();
    // A point of another sketch is not taken; the profile's is, and picking stops.
    let other = Hit::SketchPoint { sketch: frame, id: line.id.clone(), at: line.pos };
    assert!(h.app.dialog.as_ref().unwrap().candidate(&h.app.session, &other).is_none());
    let hit = Hit::SketchPoint { sketch: tube, id: id.clone(), at };
    let sel = h.app.dialog.as_ref().unwrap().candidate(&h.app.session, &hit).expect("the profile's point is taken");
    crate::viewport::dialog_pick(&mut h.app, &hit, sel);
    let p = place(&h);
    assert_eq!((p.point, p.locate, p.picking), (Some([25.0, 25.0]), crate::dialogs_weld::LOCATES.len() - 1, false));
    assert_eq!(h.app.dialog.as_ref().unwrap().inputs[1].items.len(), 1, "the skeleton keeps its line");
    h.frames(2);
    // The outline: Top left by name, then a corner of the hole as a point.
    h.step(&json!({"click": {"handle": "weld_locate:top_left"}})).unwrap();
    h.frames(2);
    assert_eq!(place(&h).locate, 1);
    h.step(&json!({"click": {"handle": "weld_locate:corner9"}})).unwrap();
    h.frames(2);
    let p = place(&h);
    let [x, y] = p.point.expect("a corner is a point");
    assert!((x.abs() - 22.0).abs() < 1e-9 && (y.abs() - 22.0).abs() < 1e-9, "{x}, {y}");
    h.step(&json!({"widget": "OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let FeatureKind::WeldMember { locate, .. } = &h.app.session.doc.features.last().unwrap().kind else { panic!() };
    assert_eq!(*locate, WeldLocate::Point([x, y]));
    assert!(h.app.session.model.results.iter().all(|r| r.error.is_none()));
}

/// The Cut List dialog's item editor: picking an item reads its description and properties,
/// Apply sets them on every body of the item, Exclude leaves it out and the Excluded list's ×
/// brings it back.
#[test]
fn the_cut_list_edits_an_items_description_and_properties() {
    let mut h = Harness::new();
    setup(&mut h);
    h.app.run("sketch.create", json!({"plane": "XY", "name": "Rect"})).unwrap();
    h.app
        .run("sketch.line", json!({"points": [[0, 1000], [950, 1000], [950, 1550], [0, 1550]], "closed": true, "ids": ["a", "b", "c", "d"]}))
        .unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app
        .run(
            "weld.member",
            json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Rect", "path": ["a", "b", "c", "d"], "body_name": "Rail"}),
        )
        .unwrap();
    h.step(&json!({"call": "ui.start", "params": {"command": "weld.cut_list"}})).unwrap();
    h.frames(3);
    // Choosing item 1 (the two long rails) reads it in.
    edit(&mut h).bodies = vec!["Rail".into()];
    h.frames(2);
    assert_eq!(edit(&mut h).bodies, ["Rail", "Rail (3)"]);
    assert_eq!(edit(&mut h).description, "", "no description of its own: the stock's name shows as the hint");
    edit(&mut h).description = "Long rail".into();
    edit(&mut h).new_name = "Mark".into();
    edit(&mut h).new_value = "A".into();
    h.frames(1);
    h.step(&json!({"widget": "Apply"})).unwrap();
    h.frames(3);
    for b in ["Rail", "Rail (3)"] {
        let c = &h.app.session.doc.cut_list[b];
        assert_eq!((c.description.as_str(), c.properties.get("Mark").map(String::as_str)), ("Long rail", Some("A")), "{b}");
    }
    assert_eq!(edit(&mut h).properties, [("Mark".to_string(), "A".to_string())], "read again after Apply");
    // Emptying the property removes it.
    edit(&mut h).properties[0].1.clear();
    h.frames(1);
    h.step(&json!({"widget": "Apply"})).unwrap();
    h.frames(3);
    assert!(h.app.session.doc.cut_list["Rail"].properties.is_empty());
    h.step(&json!({"widget": "Exclude from cut list"})).unwrap();
    h.frames(3);
    assert!(h.app.session.doc.cut_list["Rail (3)"].exclude);
    assert!(h.widget("Excluded").is_some());
    let Kind::CutList { data: Some(Ok(v)), .. } = &h.app.dialog.as_ref().unwrap().kind else { panic!("no list") };
    assert_eq!(v["members"], 2);
    h.step(&json!({"click": {"handle": "dialog:list:cut_excluded:Rail:remove"}})).unwrap();
    h.frames(3);
    assert!(!h.app.session.doc.cut_list["Rail"].exclude);
}

fn edit(h: &mut Harness) -> &mut crate::dialogs_weld::CutEdit {
    let Kind::CutList { edit, .. } = &mut h.app.dialog.as_mut().unwrap().kind else { panic!("not the cut list") };
    edit
}

/// The browser's Cut List folder lists each item with its bodies, a sub-weldment as a folder of
/// its own items, and its menu renames and dissolves it.
#[test]
fn the_browser_cut_list_shows_items_and_sub_weldments() {
    let mut h = Harness::new();
    setup(&mut h);
    h.app.run("sketch.create", json!({"plane": "XY", "name": "Rect"})).unwrap();
    h.app
        .run("sketch.line", json!({"points": [[0, 1000], [950, 1000], [950, 1550], [0, 1550]], "closed": true, "ids": ["a", "b", "c", "d"]}))
        .unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app
        .run(
            "weld.member",
            json!({"sketch": "Tube", "profiles": [{"point": [23.5, 0]}], "path_sketch": "Rect", "path": ["a", "b", "c", "d"], "body_name": "Rail"}),
        )
        .unwrap();
    let made = h.app.run("weld.sub_weldment", json!({"bodies": ["Rail (3)", "Rail (4)"], "name": "Top"})).unwrap();
    let id = made["sub_weldment"].as_u64().unwrap();
    h.app.ui.show_browser = true;
    h.app.tree.expanded.insert("c0/cut_list".into());
    h.app.tree.expanded.insert("cl:i:Rail (3)".into());
    h.frames(3);
    let labels: Vec<String> = crate::browser::drawn_rows().into_iter().map(|r| r.0).collect();
    let at = |text: &str| labels.iter().position(|l| l.contains(text)).unwrap_or_else(|| panic!("no `{text}` row in {labels:?}"));
    // The weldment's own items, then the sub-weldment's folder, its items and (open) its bodies.
    assert!(labels.iter().any(|l| l.starts_with("1 · ") && l.ends_with("(×1)")), "{labels:?}");
    let top = at("Top (2)");
    assert!(at("3 · ") > top && at("4 · ") > top, "{labels:?}");
    assert!(labels.iter().skip(top).any(|l| l == "Rail (3)"), "item 3 is open: {labels:?}");
    // Its menu: rename and dissolve.
    let items = crate::context_menu::items(&h.app, &crate::context_menu::Target::SubWeldment { id });
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert!(ids.contains(&"ui.rename") && ids.contains(&"weld.dissolve_sub_weldment"), "{ids:?}");
    // A member's own menu offers to leave it.
    let body = crate::context_menu::items(&h.app, &crate::context_menu::Target::Body { name: "Rail (4)".into() });
    assert!(body.iter().any(|i| i.label == "weld.remove_from_sub_weldment" && i.enabled), "{body:?}");
    h.app.run("weld.dissolve_sub_weldment", json!({"sub_weldment": id})).unwrap();
    h.frames(3);
    let labels: Vec<String> = crate::browser::drawn_rows().into_iter().map(|r| r.0).collect();
    assert!(!labels.iter().any(|l| l.starts_with("Top (")), "the sub-weldment's folder is gone (`Top` alone is a sketch): {labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("1 · ") && l.ends_with("(×2)")), "the rows merge again: {labels:?}");
}
