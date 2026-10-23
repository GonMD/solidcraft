//! The weldment's Cut List folder in the browser: a folder per cut list item holding its bodies,
//! the sub-weldments as folders of their own items, and the bodies left out. The list is
//! `weld.cut_list`'s, computed when the folder is open (off the window thread when the app
//! applies commands there) and kept until the design changes.

use super::*;
use crate::i18n::tr;

/// The cut list the folder shows.
#[derive(Default)]
pub(crate) struct Cache {
    /// The list for (design revision, parameters).
    list: Option<(u64, Value, Result<Value, String>)>,
    /// Computing off the window thread for (design revision, parameters).
    pending: Option<(u64, Value, std::sync::mpsc::Receiver<Result<Value, String>>)>,
}

/// The cut list for the design as it is now, or the last one while a newer one computes (none
/// before the first is in).
fn list(app: &mut SolveApp, ctx: &egui::Context) -> Option<Result<Value, String>> {
    let params = crate::dialogs_weld::params(app, false);
    let now = (app.session.revision, params);
    if let Some((r, p, rx)) = &app.tree.cut_list.pending {
        match rx.try_recv() {
            Ok(v) => {
                let (r, p) = (*r, p.clone());
                app.tree.cut_list.list = Some((r, p, v));
                app.tree.cut_list.pending = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if (*r, p) == (now.0, &now.1) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return app.tree.cut_list.list.as_ref().map(|l| l.2.clone());
            }
            Err(_) => app.tree.cut_list.pending = None,
        }
    }
    if let Some((r, p, v)) = &app.tree.cut_list.list
        && (*r, p) == (now.0, &now.1)
    {
        return Some(v.clone());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if app.apply_off_thread.is_some() && app.tree.cut_list.pending.is_none() {
        let ahead = app.session.ahead();
        let (tx, rx) = std::sync::mpsc::channel();
        let command = vec![("weld.cut_list".to_string(), now.1.clone())];
        let spawned = std::thread::Builder::new().name("cut list".into()).spawn(move || {
            let reply = ahead.replies(&command).into_iter().next().unwrap_or_else(|| Err(solidcraft_engine::EngineError::Other("no reply".into())));
            let _ = tx.send(reply.map_err(|e| e.to_string()));
        });
        if spawned.is_ok() {
            app.tree.cut_list.pending = Some((now.0, now.1, rx));
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return app.tree.cut_list.list.as_ref().map(|l| l.2.clone());
        }
    }
    let v = app.session.execute("weld.cut_list", &now.1).map_err(|e| e.to_string());
    app.tree.cut_list.list = Some((now.0, now.1, v.clone()));
    Some(v)
}

/// A cut list item's row label: its number, what it is, its size and how many.
fn item_label(app: &SolveApp, r: &Value) -> String {
    let mm = |key: &str| crate::prefs::show_mm(app, r[key].as_f64().unwrap_or(0.0), 1);
    let description = r["description"].as_str().filter(|d| !d.is_empty());
    let what = if r.get("length_mm").is_some() && r.get("end1").is_some() {
        format!("{} · {}", description.unwrap_or_default(), mm("length_mm"))
    } else if r.get("thickness_mm").is_some() && r.get("area_mm2").is_some() {
        let kind = if r["kind"] == "gusset" { "cmd.weld_gusset" } else { "cmd.weld_end_cap" };
        format!("{} · {}", description.unwrap_or(tr(kind)), mm("thickness_mm"))
    } else {
        let first = r["bodies"].get(0).and_then(Value::as_str).unwrap_or_default();
        format!("{} · {} × {} × {}", description.unwrap_or(first), mm("length_mm"), mm("width_mm"), mm("thickness_mm"))
    };
    format!("{} · {} (×{})", r["item"].as_u64().unwrap_or(0), what, r["qty"].as_u64().unwrap_or(0))
}

/// The bodies of a cut list item (a member row's `members`, a plate's or other body's `bodies`).
fn item_bodies(r: &Value) -> Vec<String> {
    let key = if r.get("members").is_some() { "members" } else { "bodies" };
    r[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// The Cut List folder of the design, while it has structural members.
pub(super) fn rows(app: &mut SolveApp, ui: &mut egui::Ui, depth: usize, acts: &mut Actions) {
    if app.session.model.state().welds.is_empty() {
        return;
    }
    let key = "c0/cut_list";
    let open = is_open(app, key, false);
    let r = draw_row(ui, ui.id().with(key), &Row { depth, fold: Some(open), icon: "bendtable", label: "Cut List", ..Default::default() });
    if r.fold || r.clicked {
        toggle(app, key, false);
    }
    if let Some(p) = r.secondary {
        acts.menu = Some((p, Target::CutList));
    }
    // Bodies dropped on the folder leave their sub-weldments.
    if let Some(resp) = &r.resp
        && let Some(d) = drop_target(ui, resp, |d: &Drag| d.folder == "bodies", DropMark::Box)
    {
        acts.drop = Some(DropAction::SubWeldment { id: None, keys: d.keys.clone() });
    }
    if !open {
        return;
    }
    let list = match list(app, ui.ctx()) {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            draw_row(
                ui,
                ui.id().with("cut_list_error"),
                &Row { depth: depth + 1, icon: "bendtable", label: &e, name: true, dim: true, ..Default::default() },
            );
            return;
        }
        None => {
            draw_row(
                ui,
                ui.id().with("cut_list_busy"),
                &Row { depth: depth + 1, icon: "bendtable", label: "weld.computing", dim: true, ..Default::default() },
            );
            return;
        }
    };
    let items: Vec<&Value> = ["rows", "plates", "bodies"].iter().flat_map(|k| list[*k].as_array().into_iter().flatten()).collect();
    let in_section = |name: &Value| -> Vec<&Value> { items.iter().copied().filter(|r| r["sub_weldment"] == *name).collect() };
    for r in in_section(&Value::Null) {
        item_rows(app, ui, r, depth + 1, acts);
    }
    for w in list["sub_weldments"].as_array().into_iter().flatten() {
        let Some(id) = w["id"].as_u64() else { continue };
        let name = w["name"].as_str().unwrap_or_default();
        let wkey = format!("cl:w{id}");
        let wopen = is_open(app, &wkey, true);
        let bodies: Vec<String> = w["bodies"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        let lit = bodies.iter().any(|b| acts.lit.thing(&Thing::Body(b.clone())) != (false, false));
        let label = format!("{name} ({})", w["items"].as_array().map_or(0, Vec::len));
        let r = draw_row(
            ui,
            ui.id().with(&wkey),
            &Row {
                depth: depth + 1,
                fold: Some(wopen),
                icon: "folder",
                label: &label,
                name: true,
                mark: if lit { Mark::Branch } else { Mark::None },
                ..Default::default()
            },
        );
        if r.fold || r.clicked {
            toggle(app, &wkey, true);
        }
        if r.hovered {
            acts.hover_bodies = bodies.clone();
        }
        if let Some(p) = r.secondary {
            acts.menu = Some((p, Target::SubWeldment { id }));
        }
        if let Some(resp) = &r.resp
            && let Some(d) = drop_target(ui, resp, |d: &Drag| d.folder == "bodies", DropMark::Box)
        {
            acts.drop = Some(DropAction::SubWeldment { id: Some(id), keys: d.keys.clone() });
        }
        if wopen {
            for r in in_section(&w["name"]) {
                item_rows(app, ui, r, depth + 2, acts);
            }
        }
    }
    let excluded: Vec<String> = list["excluded"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if !excluded.is_empty() {
        let xkey = "cl:excluded";
        let xopen = is_open(app, xkey, false);
        let label = format!("{} ({})", tr("Excluded"), excluded.len());
        let r = draw_row(
            ui,
            ui.id().with(xkey),
            &Row { depth: depth + 1, fold: Some(xopen), icon: "folder", label: &label, name: true, ..Default::default() },
        );
        if r.fold || r.clicked {
            toggle(app, xkey, false);
        }
        if r.hovered {
            acts.hover_bodies = excluded.clone();
        }
        if xopen {
            for b in &excluded {
                body_row(app, ui, b, depth + 2, acts);
            }
        }
    }
}

/// A cut list item's row (folded by default) and, open, its bodies.
fn item_rows(app: &mut SolveApp, ui: &mut egui::Ui, r: &Value, depth: usize, acts: &mut Actions) {
    let bodies = item_bodies(r);
    let Some(first) = bodies.first() else { return };
    let key = format!("cl:i:{first}");
    let open = is_open(app, &key, false);
    let label = item_label(app, r);
    let lit = bodies.iter().any(|b| acts.lit.thing(&Thing::Body(b.clone())) != (false, false));
    let row = draw_row(
        ui,
        ui.id().with(&key),
        &Row {
            depth,
            fold: Some(open),
            icon: "body",
            label: &label,
            name: true,
            mark: if lit { Mark::Branch } else { Mark::None },
            ..Default::default()
        },
    );
    if row.fold {
        toggle(app, &key, false);
    }
    // A click picks the item's bodies.
    if row.clicked {
        let items: Vec<Sel> = bodies.iter().map(|b| Sel::Body { name: b.clone() }).collect();
        let add = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let _ = app.run("select.set", json!({ "items": items, "add": add }));
    }
    if row.hovered {
        acts.hover_bodies = bodies.clone();
    }
    if let Some(p) = row.secondary {
        let items: Vec<Sel> = bodies.iter().map(|b| Sel::Body { name: b.clone() }).collect();
        let _ = app.run("select.set", json!({ "items": items }));
        acts.menu = Some((p, Target::Viewport));
    }
    if let Some(resp) = &row.resp
        && resp.drag_started()
        && !row.on_control
    {
        resp.dnd_set_drag_payload(Drag { comp: 0, folder: "bodies", keys: bodies.clone() });
    }
    if open {
        for b in &bodies {
            body_row(app, ui, b, depth + 1, acts);
        }
    }
}

/// A body's row under its cut list item: picks, shows or hides it, and drags to a sub-weldment.
fn body_row(app: &mut SolveApp, ui: &mut egui::Ui, name: &str, depth: usize, acts: &mut Actions) {
    let e = body_entry(app, name);
    app.tree.order.push(e.row_key.clone());
    let (selected, mark) = lit_row(acts.lit.entry("bodies", &e));
    let r = draw_row(
        ui,
        ui.id().with(("cut_list_body", name)),
        &Row { depth, eye: Some(e.visible), icon: e.icon, label: &e.label, name: true, selected, dim: !e.visible, mark, ..Default::default() },
    );
    if r.eye {
        set_visible(app, "bodies", &[&e], !e.visible);
    }
    if r.clicked {
        click_select(app, ui, e.row_key.clone(), e.sel.clone());
    }
    if r.hovered {
        acts.hover_bodies = e.bodies.clone();
    }
    if let Some(p) = r.secondary
        && let Some(t) = e.target.clone()
    {
        if !e.selected
            && let Some(s) = &e.sel
        {
            let _ = app.run("select.set", json!({ "items": [s] }));
        }
        acts.menu = Some((p, t));
    }
    if let Some(resp) = &r.resp
        && resp.drag_started()
        && !r.on_control
    {
        let keys = if e.selected { selected_keys(app, 0, "bodies") } else { vec![name.to_string()] };
        resp.dnd_set_drag_payload(Drag { comp: 0, folder: "bodies", keys });
    }
}

/// Bodies dropped on a sub-weldment join it; dropped on the Cut List folder, they leave theirs.
pub(super) fn drop_bodies(app: &mut SolveApp, id: Option<u64>, keys: Vec<String>) {
    match id {
        Some(id) => drop(app.run("weld.sub_weldment", json!({ "bodies": keys, "sub_weldment": id }))),
        None => {
            let doc = &app.session.doc;
            let leaving: Vec<(u64, Vec<String>)> = doc
                .sub_weldments
                .iter()
                .map(|w| (w.id, keys.iter().filter(|k| w.bodies.contains(k)).cloned().collect::<Vec<_>>()))
                .filter(|(_, b)| !b.is_empty())
                .collect();
            for (id, bodies) in leaving {
                let _ = app.run("weld.dissolve_sub_weldment", json!({ "sub_weldment": id, "bodies": bodies }));
            }
        }
    }
}

/// The menu of the Cut List folder.
pub fn folder_items(app: &SolveApp) -> Vec<Item> {
    let selected: Vec<String> =
        app.session.selection.iter().filter_map(|s| if let Sel::Body { name } = s { Some(name.clone()) } else { None }).collect();
    vec![
        crate::context_menu::cmd(app, "weld.cut_list", "Cut List"),
        crate::context_menu::cmd(app, "weld.export_cut_list", "Export Cut List"),
        Item::sep(),
        crate::context_menu::cmd(app, "weld.sub_weldment", "Create Sub-weldment").with(json!({ "bodies": selected })).on(!selected.is_empty()),
    ]
}

/// The menu of a sub-weldment's row.
pub fn sub_weldment_items(app: &SolveApp, id: u64) -> Vec<Item> {
    let Some(w) = app.session.doc.sub_weldments.iter().find(|w| w.id == id) else { return Vec::new() };
    vec![
        Item::action("ui.rename", "Rename", "rename").with(json!({ "sub_weldment": id })),
        crate::context_menu::cmd(app, "weld.dissolve_sub_weldment", "Dissolve Sub-weldment").with(json!({ "sub_weldment": id })),
        Item::sep(),
        Item::action("ui.isolate", "Isolate", "body").with(json!({ "bodies": w.bodies })),
    ]
}
