//! Weldment dialogs: the Cut List (the structural members grouped as they are cut from stock,
//! the plates and the other bodies, read from `weld.cut_list`, its export, and each item's
//! description and properties, set with `weld.cut_properties`).

use serde_json::{Value, json};

use solidcraft_engine::doc::{WeldCorner, WeldJoint, WeldLocate, WeldMirror};
use solidcraft_engine::geom::{Aabb2, Region2, Vec2, Vec3};

use crate::SolveApp;
use crate::dialogs::{Kind, check, combo, form_row};
use crate::i18n::tr;
use crate::selection::{CURVES, SelInput, SelectionExt};
use crate::ui_kit;
use crate::ui_kit::{choice_row, expr_row, hint_row, text_row};

/// Most rows the dialog lists (the export has them all).
const SHOWN_ROWS: usize = 200;

/// An end cut as the saw reads it: square, one angle, or mitre and bevel.
fn end_text(e: &Value) -> String {
    match e["cut"].as_str() {
        Some("mitre") => format!("{:.1}°", e["angle_deg"].as_f64().unwrap_or(0.0)),
        Some("compound") => format!("{:.1}° × {:.1}°", e["mitre_deg"].as_f64().unwrap_or(0.0), e["bevel_deg"].as_f64().unwrap_or(0.0)),
        _ => "square".into(),
    }
}

/// The Cut List dialog's rows: the list, kept up to date with the design, and the export buttons.
pub fn cut_list_rows(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    data: &mut Option<Result<Value, String>>,
    key: &mut Option<(u64, Value)>,
    status: &mut Option<String>,
    together: &mut bool,
    edit: &mut CutEdit,
) {
    form_row(ui, "Mirror images together", |ui| check(ui, together));
    let params = params(app, *together);
    let now = Some((app.session.revision, params));
    if *key != now {
        *data = now.as_ref().map(|(_, p)| app.session.execute("weld.cut_list", p).map_err(|e| e.to_string()));
        *key = now;
    }
    match data {
        Some(Ok(v)) => {
            let rows = v["rows"].as_array().cloned().unwrap_or_default();
            let plates = v["plates"].as_array().cloned().unwrap_or_default();
            let others = v["bodies"].as_array().cloned().unwrap_or_default();
            if rows.is_empty() && plates.is_empty() && others.is_empty() {
                ui_kit::text(ui, tr("No structural members"), ui_kit::Text::Note("cut_list"));
                return;
            }
            // Bodies without a material weigh as the default one, far lighter than steel.
            if rows.iter().chain(&plates).chain(&others).any(|r| r["material"] == "Default") {
                ui_kit::text(ui, tr("Masses use the default material: set one in Manage → Material"), ui_kit::Text::Note("cut_list_material"));
            }
            form_row(ui, "Members", |ui| ui.label(format!("{}", v["members"].as_u64().unwrap_or(0))));
            let mut section = Value::Null;
            for r in rows.iter().take(SHOWN_ROWS) {
                section_heading(ui, &mut section, r);
                let item = format!("{}  × {}", r["item"].as_u64().unwrap_or(0), r["qty"].as_u64().unwrap_or(0));
                form_row(ui, &item, |ui| {
                    ui.label(format!(
                        "{}  {}",
                        r["description"].as_str().unwrap_or(""),
                        crate::prefs::show_mm(app, r["length_mm"].as_f64().unwrap_or(0.0), 1)
                    ))
                });
                let ends = format!(
                    "{} / {}  ·  {:.2} kg each  ·  {}",
                    end_text(&r["end1"]),
                    end_text(&r["end2"]),
                    r["mass_kg"].as_f64().unwrap_or(0.0),
                    r["material"].as_str().unwrap_or("")
                );
                ui_kit::text(ui, &ends, ui_kit::Text::Note("cut_list_row"));
                bend_notes(app, ui, r);
                properties_note(ui, r);
                if let Some(item) = r["mirror_of"].as_u64() {
                    ui_kit::text(ui, &crate::i18n::format("weld.mirror_of", &[("item", &item.to_string())]), ui_kit::Text::Note("cut_list_row"));
                }
                let mirrored: Vec<&str> = r["mirrored"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if !mirrored.is_empty() {
                    let text = crate::i18n::format("weld.mirrored", &[("members", &mirrored.join(", "))]);
                    ui_kit::text(ui, &text, ui_kit::Text::Note("cut_list_row"));
                }
            }
            if rows.len() > SHOWN_ROWS {
                ui_kit::text(ui, &format!("… {} more rows in the export", rows.len() - SHOWN_ROWS), ui_kit::Text::Note("cut_list"));
            }
            let stock = v["stock"].as_array().cloned().unwrap_or_default();
            if let Some(message) = stock_rows(app, ui, &stock, edit, key.as_ref().map_or(0, |k| k.0)) {
                *status = Some(message);
            }
            plate_rows(app, ui, &plates, v["plate_count"].as_u64().unwrap_or(0));
            body_rows(app, ui, &others, v["body_count"].as_u64().unwrap_or(0));
            let welds = v["welds"].as_array().cloned().unwrap_or_default();
            weld_rows(app, ui, &welds, v["weld_count"].as_u64().unwrap_or(0), v["weld_length_mm"].as_f64().unwrap_or(0.0));
            let items: Vec<CutItem> = rows.iter().chain(&plates).chain(&others).map(CutItem::of).collect();
            if let Some(message) = item_editor(app, ui, &items, edit, key.as_ref().map_or(0, |k| k.0)) {
                *status = Some(message);
            }
            let excluded: Vec<&str> = v["excluded"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if !excluded.is_empty() {
                form_row(ui, "Excluded", |ui| ui.label(format!("{}", excluded.len())));
                for b in excluded {
                    if ui_kit::list_row(ui, &format!("cut_excluded:{b}"), b).clicked() {
                        *status = app.run("weld.cut_properties", json!({"bodies": [b], "exclude": false})).err();
                    }
                }
            }
            ui.horizontal(|ui| {
                for (label, format) in [("Export CSV…", "csv"), ("Export JSON…", "json")] {
                    if ui_kit::button(ui, tr(label), ui_kit::Button::Normal).clicked() {
                        *status = Some(export(app, format, *together));
                    }
                }
            });
            if let Some(s) = status {
                ui_kit::text(ui, s.as_str(), ui_kit::Text::Note("cut_list_status"));
            }
        }
        Some(Err(e)) => {
            ui_kit::text(ui, e.as_str(), ui_kit::Text::Error("cut_list"));
        }
        None => {}
    }
}

/// A heading before the first row of each sub-weldment's section (rows come section by section,
/// the weldment's own first).
fn section_heading(ui: &mut egui::Ui, last: &mut Value, r: &Value) {
    if r["sub_weldment"] == *last {
        return;
    }
    *last = r["sub_weldment"].clone();
    if let Some(name) = last.as_str() {
        let text = crate::i18n::format("weld.sub_weldment_section", &[("name", name)]);
        ui_kit::text(ui, &text, ui_kit::Text::Heading("cut_list_section"));
    }
}

/// A row's free properties as a note line (`Grade: S355  ·  Finish: galvanised`), if it has any.
fn properties_note(ui: &mut egui::Ui, r: &Value) {
    let Some(props) = r["properties"].as_object().filter(|m| !m.is_empty()) else { return };
    let text = props.iter().map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join("  ·  ");
    ui_kit::text(ui, &text, ui_kit::Text::Note("cut_list_row"));
}

/// A cut list item (a member row or a plate row) as the editor needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct CutItem {
    pub item: u64,
    pub bodies: Vec<String>,
    /// The description shown, and what it is when none is set (the stock's name, or nothing).
    pub description: String,
    pub fallback: String,
    pub properties: Vec<(String, String)>,
    /// A member row's length, or a plate row's thickness (mm), to tell like items apart.
    pub size: f64,
}

impl CutItem {
    fn of(r: &Value) -> CutItem {
        let names = |key: &str| r[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        let mut bodies = names("members");
        bodies.extend(names("bodies"));
        CutItem {
            item: r["item"].as_u64().unwrap_or(0),
            bodies,
            description: r["description"].as_str().unwrap_or("").to_string(),
            fallback: r["profile"].as_str().unwrap_or("").to_string(),
            properties: r["properties"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect(),
            size: r["length_mm"].as_f64().or_else(|| r["thickness_mm"].as_f64()).unwrap_or(0.0),
        }
    }

    fn label(&self, app: &SolveApp) -> String {
        let text = if self.description.is_empty() { self.bodies.first().map_or("", String::as_str) } else { &self.description };
        format!("{}  {text}  {}", self.item, crate::prefs::show_mm(app, self.size, 1))
    }
}

/// The Cut List dialog's item editor: which item, and drafts of its description and properties
/// (an empty value removes a property), applied to every body of the item at once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CutEdit {
    /// The item's bodies (items are renumbered as the list changes; its bodies find it again).
    pub bodies: Vec<String>,
    pub description: String,
    pub properties: Vec<(String, String)>,
    pub new_name: String,
    pub new_value: String,
    /// The design revision the drafts were read at.
    pub read_at: Option<u64>,
    /// Drafts of each stock's lengths (stock name, lengths as typed), and the revision they
    /// were read at.
    pub stock: Vec<(String, String)>,
    pub stock_read_at: Option<u64>,
}

impl CutEdit {
    fn load(&mut self, item: &CutItem, revision: u64) {
        let description = if item.description == item.fallback { String::new() } else { item.description.clone() };
        let (stock, stock_read_at) = (std::mem::take(&mut self.stock), self.stock_read_at);
        *self = CutEdit {
            bodies: item.bodies.clone(),
            description,
            properties: item.properties.clone(),
            read_at: Some(revision),
            stock,
            stock_read_at,
            ..CutEdit::default()
        };
    }

    /// No item chosen: the item drafts go, the stock drafts stay.
    fn clear_item(&mut self) {
        let (stock, stock_read_at) = (std::mem::take(&mut self.stock), self.stock_read_at);
        *self = CutEdit { stock, stock_read_at, ..CutEdit::default() };
    }

    /// `weld.cut_properties`' parameters for what changed from `item`, or `None` when nothing did.
    pub fn changes(&self, item: &CutItem) -> Option<Value> {
        let mut p = json!({"bodies": item.bodies});
        let was = if item.description == item.fallback { "" } else { item.description.as_str() };
        if self.description.trim() != was {
            p["description"] = json!(self.description.trim());
        }
        let mut props = serde_json::Map::new();
        for (name, value) in &self.properties {
            if item.properties.iter().any(|(n, v)| n == name && v != value.trim()) {
                props.insert(name.clone(), json!(value.trim()));
            }
        }
        if !self.new_name.trim().is_empty() && !self.new_value.trim().is_empty() {
            props.insert(self.new_name.trim().to_string(), json!(self.new_value.trim()));
        }
        if !props.is_empty() {
            p["properties"] = Value::Object(props);
        }
        (p.as_object().map_or(0, serde_json::Map::len) > 1).then_some(p)
    }
}

/// The stock rows: each stock's total, the lengths it is bought in (typed as a list, Enter
/// applies them with `weld.stock_lengths`; empty goes back to the library profile's), the bars to
/// buy and the members too long for any of them. Returns a message for the status line.
fn stock_rows(app: &mut SolveApp, ui: &mut egui::Ui, stock: &[Value], edit: &mut CutEdit, revision: u64) -> Option<String> {
    let names: Vec<&str> = stock.iter().map(|x| x["description"].as_str().unwrap_or("")).collect();
    if edit.stock_read_at != Some(revision) || edit.stock.iter().map(|(n, _)| n.as_str()).ne(names.iter().copied()) {
        edit.stock = stock
            .iter()
            .map(|x| {
                let lengths = x["stock_lengths_mm"].as_array().into_iter().flatten().filter_map(Value::as_f64);
                let text = lengths.map(|l| crate::prefs::show_mm_bare(app, l)).collect::<Vec<_>>().join(", ");
                (x["description"].as_str().unwrap_or("").to_string(), text)
            })
            .collect();
        edit.stock_read_at = Some(revision);
    }
    let mut message = None;
    for (x, (name, draft)) in stock.iter().zip(edit.stock.iter_mut()) {
        form_row(ui, "Stock", |ui| ui.label(format!("{name}  {}", crate::prefs::show_mm(app, x["total_length_mm"].as_f64().unwrap_or(0.0), 1))));
        if text_row(ui, "Stock lengths", draft, tr("e.g. 20 ft, 40 ft")).0 {
            let lengths: Vec<&str> = draft.split([',', ';']).map(str::trim).filter(|t| !t.is_empty()).collect();
            message = app.run("weld.stock_lengths", json!({"profile": name, "lengths": lengths})).err();
            edit.stock_read_at = None;
            // Enter applies the lengths; it doesn't also close the dialog.
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        }
        let buy: Vec<String> = x["buy"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| format!("{} × {}", b["qty"].as_u64().unwrap_or(0), crate::prefs::show_mm(app, b["length_mm"].as_f64().unwrap_or(0.0), 1)))
            .collect();
        if !buy.is_empty() {
            let drop = crate::prefs::show_mm(app, x["drop_mm"].as_f64().unwrap_or(0.0), 1);
            let text = crate::i18n::format("weld.stock_buy", &[("bars", &buy.join(", ")), ("drop", &drop)]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("cut_list_row"));
        }
        let too_long: Vec<&str> = x["too_long"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !too_long.is_empty() {
            let text = crate::i18n::format("weld.too_long", &[("members", &too_long.join(", "))]);
            ui_kit::text(ui, &text, ui_kit::Text::Error("cut_list_too_long"));
        }
    }
    message
}

/// The item editor rows: an item picker, then its description, properties, a new property, and
/// Apply / Exclude. Returns a message for the dialog's status line.
fn item_editor(app: &mut SolveApp, ui: &mut egui::Ui, items: &[CutItem], edit: &mut CutEdit, revision: u64) -> Option<String> {
    let at = edit.bodies.first().and_then(|b| items.iter().position(|i| i.bodies.contains(b)));
    let labels: Vec<String> = std::iter::once(tr("None").to_string()).chain(items.iter().map(|i| i.label(app))).collect();
    let options: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut chosen = at.map_or(0, |i| i + 1);
    choice_row(ui, "Edit item", "cut_list_edit_item", &mut chosen, &options);
    let Some(item) = chosen.checked_sub(1).and_then(|i| items.get(i)) else {
        edit.clear_item();
        return None;
    };
    // A new item, or the list changed (an Apply, an undo): read the item again.
    if Some(chosen) != at.map(|i| i + 1) || edit.read_at != Some(revision) {
        edit.load(item, revision);
    }
    let mut enter = text_row(ui, "Description", &mut edit.description, &item.fallback).0;
    for (name, value) in &mut edit.properties {
        enter |= text_row(ui, name, value, tr("empty removes it")).0;
    }
    enter |= text_row(ui, "New property", &mut edit.new_name, tr("Name")).0;
    enter |= text_row(ui, "Value", &mut edit.new_value, "").0;
    if enter {
        // Enter applies the item's changes; it doesn't also close the dialog.
        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    }
    let mut message = None;
    ui.horizontal(|ui| {
        if (ui_kit::button(ui, tr("Apply"), ui_kit::Button::Normal).clicked() || enter)
            && let Some(p) = edit.changes(item)
        {
            message = app.run("weld.cut_properties", p).err();
            edit.read_at = None;
        }
        if ui_kit::button(ui, tr("Exclude from cut list"), ui_kit::Button::Normal).clicked() {
            message = app.run("weld.cut_properties", json!({"bodies": item.bodies, "exclude": true})).err();
            edit.clear_item();
        }
    });
    message
}

/// The cut list's plates (End Caps, Gussets): their count, then a row per identical plate.
fn plate_rows(app: &SolveApp, ui: &mut egui::Ui, plates: &[Value], count: u64) {
    if plates.is_empty() {
        return;
    }
    form_row(ui, "Plates", |ui| ui.label(format!("{count}")));
    let mut section = Value::Null;
    for r in plates.iter().take(SHOWN_ROWS) {
        section_heading(ui, &mut section, r);
        let item = format!("{}  × {}", r["item"].as_u64().unwrap_or(0), r["qty"].as_u64().unwrap_or(0));
        let kind = if r["kind"] == "gusset" { "cmd.weld_gusset" } else { "cmd.weld_end_cap" };
        let mm = |key: &str| crate::prefs::show_mm(app, r[key].as_f64().unwrap_or(0.0), 1);
        let name = r["description"].as_str().filter(|d| !d.is_empty()).unwrap_or(tr(kind));
        form_row(ui, &item, |ui| ui.label(format!("{}  {}  ·  {} × {}", name, mm("thickness_mm"), mm("width_mm"), mm("height_mm"))));
        let mass = format!("{:.2} kg each  ·  {}", r["mass_kg"].as_f64().unwrap_or(0.0), r["material"].as_str().unwrap_or(""));
        ui_kit::text(ui, &mass, ui_kit::Text::Note("cut_list_row"));
        properties_note(ui, r);
    }
    if plates.len() > SHOWN_ROWS {
        ui_kit::text(ui, &format!("… {} more rows in the export", plates.len() - SHOWN_ROWS), ui_kit::Text::Note("cut_list"));
    }
}

/// The cut list's other bodies: their count, then a row per identical body with its bounding box.
fn body_rows(app: &SolveApp, ui: &mut egui::Ui, bodies: &[Value], count: u64) {
    if bodies.is_empty() {
        return;
    }
    form_row(ui, "Other bodies", |ui| ui.label(format!("{count}")));
    let mut section = Value::Null;
    for r in bodies.iter().take(SHOWN_ROWS) {
        section_heading(ui, &mut section, r);
        let item = format!("{}  × {}", r["item"].as_u64().unwrap_or(0), r["qty"].as_u64().unwrap_or(0));
        let mm = |key: &str| crate::prefs::show_mm(app, r[key].as_f64().unwrap_or(0.0), 1);
        let first = r["bodies"].get(0).and_then(Value::as_str).unwrap_or("");
        let name = r["description"].as_str().filter(|d| !d.is_empty()).unwrap_or(first);
        form_row(ui, &item, |ui| ui.label(format!("{}  ·  {} × {} × {}", name, mm("length_mm"), mm("width_mm"), mm("thickness_mm"))));
        let mass = format!("{:.2} kg each  ·  {}", r["mass_kg"].as_f64().unwrap_or(0.0), r["material"].as_str().unwrap_or(""));
        ui_kit::text(ui, &mass, ui_kit::Text::Note("cut_list_row"));
        properties_note(ui, r);
    }
    if bodies.len() > SHOWN_ROWS {
        ui_kit::text(ui, &format!("… {} more rows in the export", bodies.len() - SHOWN_ROWS), ui_kit::Text::Note("cut_list"));
    }
}

/// The cut list's weld table: the bead count and length welded, then a row per identical bead.
fn weld_rows(app: &SolveApp, ui: &mut egui::Ui, welds: &[Value], count: u64, total_mm: f64) {
    if welds.is_empty() {
        return;
    }
    form_row(ui, "Welds", |ui| ui.label(format!("{count}  ·  {}", crate::prefs::show_mm(app, total_mm, 0))));
    let mut section = Value::Null;
    for r in welds.iter().take(SHOWN_ROWS) {
        section_heading(ui, &mut section, r);
        let item = format!("W{}  × {}", r["weld"].as_u64().unwrap_or(0), r["qty"].as_u64().unwrap_or(0));
        let mm = |v: &Value| crate::prefs::show_mm(app, v.as_f64().unwrap_or(0.0), 1);
        let kind = if r["kind"] == "groove" { tr("Groove weld") } else { tr("Fillet weld") };
        form_row(ui, &item, |ui| ui.label(format!("{kind} {}", mm(&r["size_mm"]))));
        let mut notes = vec![mm(&r["length_mm"])];
        if let Some(i) = r["intermittent"].as_object() {
            notes.push(format!("{} @ {}", mm(&i["length_mm"]), mm(&i["pitch_mm"])));
        }
        if r["field"].as_bool() == Some(true) {
            notes.push(tr("Field weld").to_string());
        }
        if let Some(n) = r["note"].as_str() {
            notes.push(n.to_string());
        }
        let bodies: Vec<String> = r["bodies"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|pair| pair.as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(" + "))
            .collect();
        notes.push(bodies.join(", "));
        ui_kit::text(ui, &notes.join("  ·  "), ui_kit::Text::Note("cut_list_row"));
    }
    if welds.len() > SHOWN_ROWS {
        ui_kit::text(ui, &format!("… {} more rows in the export", welds.len() - SHOWN_ROWS), ui_kit::Text::Note("cut_list"));
    }
}

/// The cut list's parameters: the tolerances from the preferences and the mirror-image choice.
pub(crate) fn params(app: &SolveApp, together: bool) -> Value {
    let mut p = crate::prefs::cut_list_tolerances(app);
    p["mirrors"] = json!(if together { "together" } else { "separate" });
    p
}

/// Export the cut list to a file the user picks; the dialog shows how it went.
fn export(app: &mut SolveApp, format: &'static str, together: bool) -> String {
    let name = format!("{}-cut-list.{format}", app.session.doc.name);
    app.pick_save(&name, &[format], move |app, path| {
        let message = match path {
            Some(path) => match app.run("weld.export_cut_list", {
                let mut p = params(app, together);
                p["format"] = json!(format);
                p["path"] = json!(path);
                p
            }) {
                Ok(_) => {
                    app.set_status(format!("Exported {path}"), false);
                    format!("Saved {path}")
                }
                Err(e) => e,
            },
            None => "Export cancelled".into(),
        };
        if let Some(d) = app.dialog.as_mut()
            && let Kind::CutList { status, .. } = &mut d.kind
        {
            *status = Some(message);
        }
    });
    "Choose an export file…".into()
}

/// Most library profiles the Structural Member dialog lists at once (searching narrows them).
const SHOWN_PROFILES: usize = 200;

/// Where a Structural Member's profile comes from, in its dialog: a sketch profile picked in the
/// view, or a profile from the profile library, found by searching its name.
#[derive(Clone, Debug, Default)]
pub struct ProfileSource {
    pub library: bool,
    pub query: String,
    /// The type the Size list shows (`None`: all types).
    pub kind: Option<String>,
    /// The `weld.profiles` answer, and the query it was for (`None` until listed, or after the
    /// library changes).
    pub found: Option<(String, Result<Value, String>)>,
    /// The chosen library profile as `weld.member` takes it (`{name, region}`).
    pub chosen: Option<Value>,
    /// The name to save the picked sketch profile under.
    pub save_name: String,
    /// The library already has a profile of that name: Save offers to replace it.
    pub replace: bool,
    pub status: Option<String>,
}

impl ProfileSource {
    /// The chosen library profile's name.
    pub fn chosen_name(&self) -> Option<&str> {
        self.chosen.as_ref().and_then(|c| c["name"].as_str())
    }
}

/// The "Profile from" row: a sketch profile or a library profile.
pub fn profile_source_row(ui: &mut egui::Ui, src: &mut ProfileSource) {
    form_row(ui, "Profile from", |ui| {
        let mut i = usize::from(src.library);
        combo(ui, "weld_profile_from", &["Sketch", "Library"], &mut i);
        src.library = i == 1;
    });
}

/// The library rows of the Structural Member dialog: in library mode the search and the profiles
/// found (a click chooses one); otherwise saving the picked sketch profile (`picked`: its sketch
/// and profile index) to the library under a name.
pub fn profile_rows(app: &mut SolveApp, ui: &mut egui::Ui, src: &mut ProfileSource, picked: Option<(u64, usize)>) {
    if src.library {
        if src.found.as_ref().is_none_or(|(q, _)| *q != src.query) {
            let found = app.session.execute("weld.profiles", &json!({"query": src.query})).map_err(|e| e.to_string());
            src.found = Some((src.query.clone(), found));
        }
        let listed = match src.found.as_ref().map(|(_, f)| f) {
            Some(Ok(v)) => Some((
                v["types"].as_array().cloned().unwrap_or_default(),
                v["profiles"].as_array().cloned().unwrap_or_default(),
                v["total"].as_u64().unwrap_or(0),
            )),
            Some(Err(e)) => {
                ui_kit::text(ui, e.as_str(), ui_kit::Text::Error("weld_profile"));
                None
            }
            None => None,
        };
        let mut choose = None;
        if let Some((types, profiles, total)) = listed {
            let chosen = src.chosen_name().map(str::to_string);
            // An edited member's profile opens on its type; a type the search no longer finds
            // falls back to all types.
            if src.kind.is_none()
                && let Some(name) = &chosen
            {
                src.kind = profiles.iter().find(|p| p["name"].as_str() == Some(name)).and_then(|p| p["type"].as_str()).map(str::to_string);
            }
            if src.kind.as_ref().is_some_and(|k| !types.iter().any(|t| t["type"].as_str() == Some(k))) {
                src.kind = None;
            }
            // Type: "All types" first, then each type with its count.
            let mut type_labels = vec![format!("{} ({})", tr("All types"), profiles.len())];
            type_labels.extend(types.iter().map(|ty| format!("{} ({})", ty["type"].as_str().unwrap_or(""), ty["count"].as_u64().unwrap_or(0))));
            let type_labels: Vec<&str> = type_labels.iter().map(String::as_str).collect();
            let mut ti = src.kind.as_ref().and_then(|k| types.iter().position(|t| t["type"].as_str() == Some(k))).map_or(0, |i| i + 1);
            choice_row(ui, "Type", "weld_profile_type", &mut ti, &type_labels);
            src.kind = ti.checked_sub(1).and_then(|i| types.get(i)).and_then(|t| t["type"].as_str()).map(str::to_string);
            let shown: Vec<&Value> = profiles.iter().filter(|p| src.kind.as_deref().is_none_or(|k| p["type"].as_str() == Some(k))).collect();
            if shown.is_empty() {
                let none = if total == 0 { "No profiles in the library yet" } else { "No matching profiles" };
                form_row(ui, "Size", |ui| ui_kit::text(ui, tr(none), ui_kit::Text::Note("weld_profile_size")));
            } else {
                // Within one type a profile is its size; across types, its whole name. The first
                // entry stands for no choice yet.
                let label = |p: &Value| p[if src.kind.is_some() { "size" } else { "name" }].as_str().unwrap_or("").to_string();
                let listed: Vec<&Value> = shown.iter().take(SHOWN_PROFILES).copied().collect();
                let mut size_labels = vec![chosen.clone().unwrap_or_else(|| tr("Choose a size").to_string())];
                size_labels.extend(listed.iter().map(|p| label(p)));
                let size_labels: Vec<&str> = size_labels.iter().map(String::as_str).collect();
                let mut si = listed.iter().position(|p| p["name"].as_str() == chosen.as_deref()).map_or(0, |i| i + 1);
                let before = si;
                choice_row(ui, "Size", "weld_profile_size", &mut si, &size_labels);
                if si != before
                    && let Some(p) = si.checked_sub(1).and_then(|i| listed.get(i))
                {
                    choose = p["name"].as_str().map(str::to_string);
                }
                if shown.len() > SHOWN_PROFILES {
                    let more = format!("… {} more: pick a type or search", shown.len() - SHOWN_PROFILES);
                    ui_kit::text(ui, &more, ui_kit::Text::Note("weld_profile_more"));
                }
                if let Some(p) = listed.iter().find(|p| p["name"].as_str() == chosen.as_deref()) {
                    let mm = |key: &str| crate::prefs::show_mm(app, p[key].as_f64().unwrap_or(0.0), 1);
                    let dims = format!("{} × {}  ·  {:.1} mm²", mm("width_mm"), mm("height_mm"), p["area_mm2"].as_f64().unwrap_or(0.0));
                    ui_kit::text(ui, &dims, ui_kit::Text::Note("weld_profile_dims"));
                }
            }
        }
        form_row(ui, "Search", |ui| ui_kit::text_field(ui, &mut src.query, tr("Profile name")));
        if let Some(name) = choose {
            match app.session.execute("weld.profile_get", &json!({"name": name})) {
                Ok(v) => {
                    src.chosen = Some(v["stock"].clone());
                    src.status = None;
                }
                Err(e) => src.status = Some(e.to_string()),
            }
        }
        if let Some(name) = src.chosen_name().map(str::to_string)
            && ui_kit::button(ui, tr("Delete from library"), ui_kit::Button::Normal)
                .on_hover_text(tr("Members already made keep their copy"))
                .clicked()
        {
            src.status = Some(match app.run("weld.profile_delete", json!({"name": name})) {
                Ok(_) => format!("Deleted {name} from the library"),
                Err(e) => e,
            });
            src.found = None;
        }
    } else {
        form_row(ui, "Save to library", |ui| {
            if ui_kit::text_field(ui, &mut src.save_name, tr("Profile name")).1 {
                src.replace = false;
            }
        });
        let ready = picked.is_some() && !src.save_name.trim().is_empty();
        let save = ui_kit::button(ui, tr(if src.replace { "Replace" } else { "Save" }), ui_kit::Button::Enabled(ready))
            .on_disabled_hover_text(tr("Pick a profile and give it a name"))
            .clicked();
        if save && let Some((sketch, index)) = picked {
            let name = src.save_name.trim().to_string();
            let p = json!({"name": name, "sketch": sketch, "profiles": [index], "replace": src.replace});
            src.status = Some(match app.run("weld.profile_save", p) {
                Ok(_) => {
                    src.replace = false;
                    src.found = None;
                    format!("Saved {name} to the library")
                }
                Err(e) if e.contains("already has") => {
                    src.replace = true;
                    format!("The library already has {name}: Replace saves over it")
                }
                Err(e) => e,
            });
        }
    }
    if let Some(s) = &src.status {
        ui_kit::text(ui, s.as_str(), ui_kit::Text::Note("weld_profile_status"));
    }
}

/// Most groups the Structural Member dialog adds after its Skeleton (the command takes more).
pub const MAX_GROUPS: usize = 8;

/// The group inputs' labels: the Skeleton is group 1.
const GROUP_LABELS: [&str; MAX_GROUPS] = ["Group 2", "Group 3", "Group 4", "Group 5", "Group 6", "Group 7", "Group 8", "Group 9"];

/// The points of the profile the Locate list offers, and `weld.member`'s `locate` for each; the
/// last, a point in the profile's coordinates, only for a feature that has one.
pub const LOCATES: [(&str, &str); 11] = [
    ("Centroid", "centroid"),
    ("Top left", "top_left"),
    ("Top", "top"),
    ("Top right", "top_right"),
    ("Left", "left"),
    ("Middle", "middle"),
    ("Right", "right"),
    ("Bottom left", "bottom_left"),
    ("Bottom", "bottom"),
    ("Bottom right", "bottom_right"),
    ("Point", "point"),
];

/// The Mirror list, and `weld.member`'s `mirror` for each.
pub const MIRRORS: [(&str, &str); 3] = [("None", "none"), ("Left to right", "left_right"), ("Top to bottom", "up_down")];

/// A bent member row's bends, one line each as a bender makes them, then the straight after the
/// last bend and the length of stock to cut (a member formed to a free-form shape: that, and the
/// length to cut); a ring's seam.
fn bend_notes(app: &SolveApp, ui: &mut egui::Ui, r: &Value) {
    let mm = |v: &Value| crate::prefs::show_mm(app, v.as_f64().unwrap_or(0.0), 1);
    if r["ring"].as_bool() == Some(true) {
        ui_kit::text(ui, tr("weld.ring"), ui_kit::Text::Note("cut_list_row"));
    }
    if r["formed"].as_bool() == Some(true) {
        let text = crate::i18n::format("weld.formed", &[("cut", &mm(&r["cut_length_mm"]))]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("cut_list_row"));
        return;
    }
    let Some(bends) = r["bends"].as_array().filter(|b| !b.is_empty()) else { return };
    for (i, b) in bends.iter().enumerate() {
        let deg = |key: &str| format!("{:.1}°", b[key].as_f64().unwrap_or(0.0));
        let n = (i + 1).to_string();
        let (feed, turn, angle, radius) = (mm(&b["feed_mm"]), deg("rotation_deg"), deg("angle_deg"), mm(&b["radius_mm"]));
        let text = crate::i18n::format("weld.bend", &[("n", &n), ("feed", &feed), ("turn", &turn), ("angle", &angle), ("radius", &radius)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("cut_list_row"));
    }
    let text = crate::i18n::format("weld.bent", &[("tail", &mm(&r["tail_mm"])), ("cut", &mm(&r["cut_length_mm"]))]);
    ui_kit::text(ui, &text, ui_kit::Text::Note("cut_list_row"));
}

/// The Structural Member dialog's bent member settings: whether tangent lines and arcs make one
/// member, and the stock added at each end of one for bending.
#[derive(Clone, Debug, PartialEq)]
pub struct Bends {
    pub merge: bool,
    pub allowance: String,
}

impl Default for Bends {
    fn default() -> Self {
        Bends { merge: true, allowance: String::new() }
    }
}

/// Where the profile lies on its lines: its located point and mirror, as the dialog holds them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    /// Index into [`LOCATES`].
    pub locate: usize,
    /// The feature's point in the profile's coordinates, kept while editing it.
    pub point: Option<[f64; 2]>,
    /// Index into [`MIRRORS`].
    pub mirror: usize,
    /// A click on a point of the profile's sketch in the view locates the profile there.
    pub picking: bool,
}

impl Place {
    pub fn of(locate: WeldLocate, mirror: WeldMirror) -> Place {
        let name = |v: Value| v.as_str().map(str::to_string);
        let point = match locate {
            WeldLocate::Point(p) => Some(p),
            _ => None,
        };
        let at = |list: &[(&str, &str)], v: Option<String>| list.iter().position(|(_, id)| Some(*id) == v.as_deref());
        Place {
            locate: if point.is_some() { LOCATES.len() - 1 } else { at(&LOCATES, serde_json::to_value(locate).ok().and_then(name)).unwrap_or(0) },
            point,
            mirror: at(&MIRRORS, serde_json::to_value(mirror).ok().and_then(name)).unwrap_or(0),
            picking: false,
        }
    }

    /// Located at `at` in the profile's coordinates (a point picked in the view or on the
    /// outline).
    pub fn set_point(&mut self, at: [f64; 2]) {
        self.point = Some(at);
        self.locate = LOCATES.len() - 1;
        self.picking = false;
    }

    /// `weld.member`'s `locate` (`None` for the centroid).
    pub fn locate_param(&self) -> Option<Value> {
        match LOCATES.get(self.locate).map(|(_, id)| *id) {
            None | Some("centroid") => None,
            Some("point") => self.point.map(|p| json!({"point": p})),
            Some(id) => Some(json!(id)),
        }
    }

    /// `weld.member`'s `mirror` (`None` for none).
    pub fn mirror_param(&self) -> Option<Value> {
        MIRRORS.get(self.mirror).filter(|_| self.mirror > 0).map(|(_, id)| json!(id))
    }
}

/// The Locate list's labels: "Point" only where there is one.
fn locate_labels(point: bool) -> Vec<&'static str> {
    LOCATES.iter().take(if point { LOCATES.len() } else { LOCATES.len() - 1 }).map(|(l, _)| *l).collect()
}

/// Most corners the Locate outline offers (a profile of many short segments, such as text,
/// offers the first ones).
const MAX_CORNERS: usize = 200;

/// A point the Locate outline offers, and where it is in the profile's coordinates: a Locate
/// entry (an index into [`LOCATES`]: the centroid or a point of the bounding box) or, for `None`,
/// a corner of the profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocateMark {
    pub locate: Option<usize>,
    pub at: Vec2,
}

/// The points the Locate outline offers for `region`: its centroid, the nine points of its
/// bounding box, and the corners of its outline and holes (where two segments meet at an
/// angle); a point already offered is offered once, as the first.
pub fn locate_marks(region: &Region2) -> Vec<LocateMark> {
    let b: Aabb2 = region.outer.polyline(1e-4).into_iter().collect();
    let tol = b.size().len() * 1e-6;
    const BOX: [(f64, f64); 9] = [(0.0, 1.0), (0.5, 1.0), (1.0, 1.0), (0.0, 0.5), (0.5, 0.5), (1.0, 0.5), (0.0, 0.0), (0.5, 0.0), (1.0, 0.0)];
    let named = std::iter::once(region.centroid()).chain(BOX.iter().map(|(u, v)| Vec2::new(b.min.x + b.size().x * u, b.min.y + b.size().y * v)));
    let mut marks: Vec<LocateMark> = Vec::new();
    for (i, at) in named.enumerate() {
        if !marks.iter().any(|m| m.at.dist(at) <= tol) {
            marks.push(LocateMark { locate: Some(i), at });
        }
    }
    let mut corners = 0;
    for (seg, next) in std::iter::once(&region.outer).chain(&region.holes).flat_map(|l| l.joints()) {
        let smooth = match (seg.end_tangent().normalized(), next.start_tangent().normalized()) {
            (Some(t), Some(u)) => t.cross(u).abs() < 1e-3 && t.dot(u) > 0.0,
            _ => false,
        };
        let at = next.start();
        if !smooth && at.is_finite() && !marks.iter().any(|m| m.at.dist(at) <= tol) {
            marks.push(LocateMark { locate: None, at });
            corners += 1;
            if corners == MAX_CORNERS {
                break;
            }
        }
    }
    marks
}

/// Where `place` locates the profile among `marks`: the mark of its Locate entry, or of its point.
fn located_mark(marks: &[LocateMark], place: &Place) -> Option<usize> {
    match (LOCATES.get(place.locate).map(|(_, id)| *id), place.point) {
        (Some("point"), Some([x, y])) => marks.iter().position(|m| m.locate.is_none() && m.at.dist(Vec2::new(x, y)) < 1e-9),
        (Some("point"), None) | (None, _) => None,
        _ => marks.iter().position(|m| m.locate == Some(place.locate)),
    }
}

/// The profile's own region, as the Structural Member dialog has it: the chosen library profile's,
/// or the picked sketch profile's (`picked`: its sketch and profile index).
pub fn profile_region(app: &SolveApp, source: &ProfileSource, picked: Option<(u64, usize)>) -> Option<Region2> {
    if source.library {
        return source.chosen.as_ref().and_then(|c| serde_json::from_value(c["region"].clone()).ok());
    }
    let (sketch, index) = picked?;
    app.session.world_state().sketch(sketch)?.profiles.get(index).map(|p| p.region.clone())
}

/// The profile's outline under the Locate row with the points [`locate_marks`] offers: a click on
/// one locates the profile there (by name for the centroid and the box points, a corner as a
/// point); the located point is filled. A point that isn't one of them (set by a command) is
/// drawn filled too.
fn locate_outline(app: &SolveApp, ui: &mut egui::Ui, region: &Region2, place: &mut Place) {
    let t = crate::theme::Tokens::get();
    let b: Aabb2 = region.outer.polyline(1e-4).into_iter().collect();
    let size = b.size();
    if !(size.x.max(size.y) > 1e-9) {
        return;
    }
    let width = ui.available_width().max(80.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, width.min(120.0)), egui::Sense::click());
    let inner = rect.shrink(12.0);
    let scale = (f64::from(inner.width()) / size.x.max(1e-9)).min(f64::from(inner.height()) / size.y.max(1e-9));
    let mid = b.min.lerp(b.max, 0.5);
    let c = inner.center();
    let screen = |p: Vec2| egui::pos2(c.x + ((p.x - mid.x) * scale) as f32, c.y - ((p.y - mid.y) * scale) as f32);
    let painter = ui.painter_at(rect);
    painter.rect_stroke(rect, f32::from(t.radius_sm), egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    for lp in std::iter::once(&region.outer).chain(&region.holes) {
        let pts: Vec<egui::Pos2> = lp.polyline(size.len() * 1e-3).into_iter().map(screen).collect();
        painter.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.2, t.text)));
    }
    let marks = locate_marks(region);
    let located = located_mark(&marks, place);
    let hover = resp.hover_pos().and_then(|at| {
        marks.iter().enumerate().map(|(i, m)| (i, screen(m.at).distance(at))).filter(|(_, d)| *d <= 8.0).min_by(|a, b| a.1.total_cmp(&b.1))
    });
    for (i, m) in marks.iter().enumerate() {
        let at = screen(m.at);
        let r = if m.locate.is_some() { 3.0 } else { 2.5 };
        if Some(i) == located || hover.is_some_and(|(h, _)| h == i) {
            painter.circle_filled(at, r + 1.0, t.accent);
        } else {
            painter.circle_stroke(at, r, egui::Stroke::new(1.0, t.text_dim));
        }
        let name = m.locate.and_then(|l| LOCATES.get(l)).map_or_else(|| format!("corner{i}"), |(_, id)| (*id).to_string());
        crate::scenario::publish_handle(&format!("weld_locate:{name}"), at);
    }
    if located.is_none()
        && let (Some("point"), Some([x, y])) = (LOCATES.get(place.locate).map(|(_, id)| *id), place.point)
    {
        painter.circle_filled(screen(Vec2::new(x, y)), 3.5, t.accent);
    }
    let Some(m) = hover.and_then(|(i, _)| marks.get(i)) else { return };
    let label = match m.locate.and_then(|l| LOCATES.get(l)) {
        Some((label, _)) => tr(label).to_string(),
        None => format!("{} {}, {}", tr("ui.corner"), crate::prefs::show_mm_bare(app, m.at.x), crate::prefs::show_mm_bare(app, m.at.y)),
    };
    if resp.on_hover_text(label).clicked() {
        match m.locate {
            Some(l) => {
                place.locate = l;
                place.picking = false;
            }
            None => place.set_point([m.at.x, m.at.y]),
        }
    }
}

/// `angle` turned on by `by` degrees: a number of degrees stays one (from 0 up to 360), anything
/// else gains `+ 90 deg` or `- 90 deg`.
pub fn turned(angle: &str, by: f64) -> String {
    let a = angle.trim();
    let degrees =
        if a.is_empty() { Some(0.0) } else { a.strip_suffix("deg").or_else(|| a.strip_suffix('°')).and_then(|n| n.trim().parse::<f64>().ok()) };
    match degrees.filter(|d| d.is_finite()) {
        Some(d) => {
            let t = (d + by).rem_euclid(360.0);
            let t = if (t - t.round()).abs() < 1e-9 { t.round() } else { t };
            format!("{t} deg")
        }
        None => format!("{a} {} {} deg", if by < 0.0 { "-" } else { "+" }, by.abs()),
    }
}

/// Buttons turning `angle` a quarter turn either way.
fn quarter_turns(ui: &mut egui::Ui, angle: &mut String, base: &str) {
    ui.horizontal(|ui| {
        for (label, by, hint) in [("−90°", -90.0, "Turn a quarter turn back"), ("+90°", 90.0, "Turn a quarter turn on")] {
            if ui_kit::button(ui, label, ui_kit::Button::Small).on_hover_text(tr(hint)).clicked() {
                *angle = turned(if angle.trim().is_empty() { base } else { angle }, by);
            }
        }
    });
}

/// A further group's own settings in the Structural Member dialog (its lines are the input after
/// the Skeleton's, in order).
#[derive(Clone, Debug, Default)]
pub struct GroupRow {
    /// The profile's turn for this group (empty: the feature's).
    pub angle: String,
    /// 0 the feature's corner, 1 mitre, 2 butt.
    pub corner: usize,
    /// 0 the feature's located point, else 1 + an index into [`LOCATES`].
    pub locate: usize,
    /// The group's point in the profile's coordinates, kept while editing it.
    pub point: Option<[f64; 2]>,
    /// 0 the feature's mirror, else 1 + an index into [`MIRRORS`].
    pub mirror: usize,
}

impl GroupRow {
    /// A group's own `locate` and `mirror` (where it has them), the rest left empty.
    pub fn of_place(locate: Option<WeldLocate>, mirror: Option<WeldMirror>) -> GroupRow {
        let own = Place::of(locate.unwrap_or_default(), mirror.unwrap_or_default());
        GroupRow {
            locate: locate.map_or(0, |_| own.locate + 1),
            point: own.point,
            mirror: mirror.map_or(0, |_| own.mirror + 1),
            ..Default::default()
        }
    }

    /// The group's `locate` and `mirror`, where it has its own.
    pub fn place_params(&self) -> (Option<Value>, Option<Value>) {
        let own = Place { locate: self.locate.saturating_sub(1), point: self.point, mirror: self.mirror.saturating_sub(1), picking: false };
        let locate = (self.locate > 0).then(|| own.locate_param().unwrap_or_else(|| json!("centroid")));
        let mirror = (self.mirror > 0).then(|| own.mirror_param().unwrap_or_else(|| json!("none")));
        (locate, mirror)
    }
}

/// The Structural Member dialog's skeleton groups: each further group's settings, and the sketch
/// each skeleton input's lines were picked in (the Skeleton's first), as line ids repeat across
/// sketches.
#[derive(Clone, Debug, Default)]
pub struct Groups {
    pub rows: Vec<GroupRow>,
    pub sketches: Vec<Option<u64>>,
    /// Corners joined otherwise than the feature's corner, by their members' skeleton lines.
    pub joints: Vec<LineJoint>,
    /// The `weld.corners` answer, and the design revision and parameters it was for.
    pub corners: Option<((u64, Value), Result<Value, String>)>,
    /// The corner whose row the pointer is over (world point), ringed in the view.
    pub hovered: Option<Vec3>,
}

/// A joint of the Structural Member dialog: its two members as skeleton lines (sketch, line id),
/// so it stays with them when groups or lines are added, removed or reordered.
#[derive(Clone, Debug, PartialEq)]
pub struct LineJoint {
    pub lines: [(u64, String); 2],
    pub corner: WeldCorner,
    /// For a butt, which of `lines` runs through (`None`: the first in the feature's order).
    pub through: Option<usize>,
}

impl LineJoint {
    /// The joints of Structural Member `kind`, by their members' lines.
    pub fn of(kind: &solidcraft_engine::doc::FeatureKind) -> Vec<LineJoint> {
        let solidcraft_engine::doc::FeatureKind::WeldMember { joints, .. } = kind else { return Vec::new() };
        let (lines, keys) = (solidcraft_engine::doc::weld::member_lines(kind), kind.weld_keys());
        let line = |key: &String| keys.iter().position(|k| k == key).and_then(|i| lines.get(i)).map(|(s, l)| (s.0, l.to_string()));
        joints
            .iter()
            .filter_map(|j| {
                let [a, b] = &j.members;
                let lines = [line(a)?, line(b)?];
                let through = j.through.as_ref().and_then(|t| j.members.iter().position(|m| m == t));
                Some(LineJoint { lines, corner: j.corner, through })
            })
            .collect()
    }

    /// Whether it joins the members on lines `a` and `b`.
    fn joins(&self, a: &(u64, String), b: &(u64, String)) -> bool {
        (self.lines[0] == *a && self.lines[1] == *b) || (self.lines[0] == *b && self.lines[1] == *a)
    }
}

impl Groups {
    /// A line of `sketch` picked into selection input `input` (1: the Skeleton).
    pub fn picked(&mut self, input: usize, sketch: u64) {
        let Some(i) = input.checked_sub(1) else { return };
        if self.sketches.len() <= i {
            self.sketches.resize(i + 1, None);
        }
        if let Some(slot) = self.sketches.get_mut(i) {
            *slot = Some(sketch);
        }
    }
}

/// A further group's selection input, the `n`th after the Skeleton.
pub fn group_input(n: usize) -> SelInput {
    SelInput::new(GROUP_LABELS.get(n).copied().unwrap_or("Group 9"), CURVES, true)
}

/// The further groups' rows (angle, corner, remove) and Add group: adding one opens its input for
/// picking, removing one drops its input.
pub fn group_rows(ui: &mut egui::Ui, groups: &mut Groups, inputs: &mut Vec<SelInput>, active: &mut usize, angle: &str) {
    let mut remove = None;
    for (i, g) in groups.rows.iter_mut().enumerate() {
        form_row(ui, GROUP_LABELS.get(i).copied().unwrap_or(""), |ui| {
            ui_kit::hint_field(ui, &mut g.angle, tr("same"));
        });
        form_row(ui, "", |ui| quarter_turns(ui, &mut g.angle, angle));
        form_row(ui, "", |ui| {
            ui.horizontal(|ui| {
                let same = ["Same as feature"];
                let locates: Vec<&str> = same.iter().copied().chain(locate_labels(g.point.is_some())).collect();
                combo(ui, &format!("weld_group_locate_{i}"), &locates, &mut g.locate);
                let mirrors: Vec<&str> = same.iter().copied().chain(MIRRORS.iter().map(|(l, _)| *l)).collect();
                combo(ui, &format!("weld_group_mirror_{i}"), &mirrors, &mut g.mirror);
            });
        });
        form_row(ui, "", |ui| {
            ui.horizontal(|ui| {
                combo(ui, &format!("weld_group_corner_{i}"), &["Same as feature", "Mitre", "Butt"], &mut g.corner);
                if ui_kit::button(ui, "×", ui_kit::Button::Small).on_hover_text(tr("Remove group")).clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        groups.rows.remove(i);
        if i + 2 < inputs.len() {
            inputs.remove(i + 2);
        }
        if i + 1 < groups.sketches.len() {
            groups.sketches.remove(i + 1);
        }
        for (n, inp) in inputs.iter_mut().skip(2).enumerate() {
            inp.label = GROUP_LABELS.get(n).copied().unwrap_or("Group 9");
        }
        *active = (*active).min(inputs.len().saturating_sub(1));
    }
    if groups.rows.len() < MAX_GROUPS && ui_kit::button(ui, tr("Add group"), ui_kit::Button::Normal).clicked() {
        groups.rows.push(GroupRow::default());
        inputs.push(group_input(groups.rows.len() - 1));
        *active = inputs.len() - 1;
    }
}

/// Sketch curve ids picked in input `i`.
fn curve_ids(inputs: &[SelInput], i: usize) -> Vec<String> {
    inputs.get(i).map(|inp| inp.items.curve_ids()).unwrap_or_default()
}

/// The Structural Member dialog's rows after its inputs; whether Enter was pressed in a field.
pub fn member_rows(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    (angle, place, corner, gap, join, source, groups, bends): (
        &mut String,
        &mut Place,
        &mut usize,
        &mut String,
        &mut bool,
        &mut ProfileSource,
        &mut Groups,
        &mut Bends,
    ),
    picked: Option<(u64, usize)>,
    editing: Option<u64>,
    inputs: &mut Vec<SelInput>,
    active: &mut usize,
) -> bool {
    profile_rows(app, ui, source, picked);
    let mut enter = expr_row(ui, "Angle", angle);
    form_row(ui, "", |ui| quarter_turns(ui, angle, "0 deg"));
    choice_row(ui, "Locate", "weld_locate", &mut place.locate, &locate_labels(place.point.is_some()));
    // A sketch profile's located point may also be picked among its sketch's points in the view.
    if !source.library {
        form_row(ui, "", |ui| {
            let label = if place.picking { "Stop picking" } else { "Pick in view" };
            let hint = "Click a point of the profile's sketch to lie on the lines";
            if ui_kit::button(ui, tr(label), ui_kit::Button::Enabled(picked.is_some())).on_hover_text(tr(hint)).clicked() {
                place.picking = !place.picking;
            }
        });
        place.picking &= picked.is_some();
        if place.picking {
            ui_kit::text(ui, tr("Click a point of the profile's sketch"), ui_kit::Text::Hint("weld_locate_pick"));
        }
    }
    if let Some(region) = profile_region(app, source, picked) {
        locate_outline(app, ui, &region, place);
    }
    let mirrors: Vec<&str> = MIRRORS.iter().map(|(l, _)| *l).collect();
    choice_row(ui, "Mirror", "weld_mirror", &mut place.mirror, &mirrors);
    choice_row(ui, "Corner", "weld_corner", corner, &["Mitre", "Butt"]);
    enter |= hint_row(ui, "Gap", gap, tr("none"));
    form_row(ui, "Join earlier members", |ui| check(ui, join));
    form_row(ui, "Merge tangent arcs", |ui| check(ui, &mut bends.merge));
    enter |= hint_row(ui, "End allowance", &mut bends.allowance, tr("none"));
    group_rows(ui, groups, inputs, active, angle);
    let not = if source.library { None } else { picked.map(|(sketch, _)| sketch) };
    corner_rows(app, ui, inputs, groups, *corner, not, editing);
    enter
}

/// The Trim/Extend dialog's rows.
pub fn trim_rows(ui: &mut egui::Ui, mode: &mut usize, gap: &mut String, cope: &mut bool) -> bool {
    choice_row(ui, "Mode", "weld_trim_mode", mode, &["Trim and extend", "Trim only", "Extend only"]);
    let enter = hint_row(ui, "Gap", gap, tr("none"));
    form_row(ui, "Cope to fit", |ui| check(ui, cope));
    enter
}

/// The End Cap dialog's rows.
pub fn end_cap_rows(
    ui: &mut egui::Ui,
    thickness: &mut String,
    inset: &mut String,
    inside: &mut bool,
    corner: &mut usize,
    corner_size: &mut String,
) -> bool {
    let mut enter = expr_row(ui, "Thickness", thickness);
    enter |= hint_row(ui, "Inset", inset, tr("none"));
    form_row(ui, "Inside", |ui| check(ui, inside));
    choice_row(ui, "Corners", "weld_cap_corner", corner, &["None", "Chamfer", "Fillet"]);
    match *corner {
        1 => enter |= expr_row(ui, "Distance", corner_size),
        2 => enter |= expr_row(ui, "Radius", corner_size),
        _ => {}
    }
    enter
}

/// The Gusset dialog's rows: sizes, thickness and its side, offset, and which way each leg runs.
#[allow(clippy::too_many_arguments)]
pub fn gusset_rows(
    ui: &mut egui::Ui,
    size: &mut String,
    size2: &mut String,
    thickness: &mut String,
    edge: &mut String,
    clip: &mut String,
    offset: &mut String,
    side: &mut usize,
    reverse: &mut [bool; 2],
) -> bool {
    let mut enter = expr_row(ui, "Size", size);
    enter |= hint_row(ui, "Second size", size2, tr("same"));
    enter |= hint_row(ui, "Edge height", edge, tr("none"));
    enter |= hint_row(ui, "Corner clip", clip, tr("none"));
    enter |= expr_row(ui, "Thickness", thickness);
    choice_row(ui, "Thickness side", "weld_gusset_side", side, &["Centred", "One side", "Other side"]);
    enter |= hint_row(ui, "Offset", offset, tr("none"));
    for (label, value) in ["Reverse first leg", "Reverse second leg"].into_iter().zip(reverse.iter_mut()) {
        form_row(ui, label, |ui| check(ui, value));
    }
    enter
}

/// The Weld Bead dialog's rows: size and kind, root gap, intermittent welds, field weld and note.
#[allow(clippy::too_many_arguments)]
pub fn bead_rows(
    ui: &mut egui::Ui,
    size: &mut String,
    kind: &mut usize,
    gap: &mut String,
    intermittent: &mut bool,
    length: &mut String,
    pitch: &mut String,
    field: &mut bool,
    note: &mut String,
) -> bool {
    let mut enter = expr_row(ui, "Size", size);
    choice_row(ui, "Type", "weld_bead_kind", kind, &["Fillet weld", "Groove weld"]);
    enter |= hint_row(ui, "Gap", gap, tr("none"));
    form_row(ui, "Intermittent", |ui| check(ui, intermittent));
    if *intermittent {
        enter |= expr_row(ui, "Length", length);
        enter |= expr_row(ui, "Pitch", pitch);
    }
    form_row(ui, "Field weld", |ui| check(ui, field));
    enter |= text_row(ui, "Note", note, "").0;
    enter
}

/// The sketch of skeleton input `input`'s lines `ids`: the one they were picked in while it has
/// them all, else one that has them (not `not`, the profile's).
fn skeleton_sketch(app: &SolveApp, groups: &Groups, input: usize, ids: &[String], not: Option<u64>) -> Option<u64> {
    let st = app.session.world_state();
    let has =
        |sid: u64| st.sketch(sid).is_some_and(|ss| ids.iter().all(|id| ss.sketch.curve_index(id).is_some() || ss.sketch.wire_index(id).is_some()));
    groups.sketches.get(input.wrapping_sub(1)).copied().flatten().filter(|s| has(*s)).or_else(|| crate::dialogs::curves_sketch(app, ids, not))
}

/// `weld.member`'s skeleton: `path_sketch`, `path` and `groups` from the Skeleton and group
/// inputs, and `joints` from the dialog's joints at its members' keys now (a joint of a member no
/// longer there dropped).
pub fn skeleton_params(app: &SolveApp, inputs: &[SelInput], groups: &Groups, not: Option<u64>) -> Result<serde_json::Map<String, Value>, String> {
    skeleton(app, inputs, groups, not).map(|(p, _, _)| p)
}

/// [`skeleton_params`], and each member's line (sketch, line id) and key, in key order.
fn skeleton(
    app: &SolveApp,
    inputs: &[SelInput],
    groups: &Groups,
    not: Option<u64>,
) -> Result<(serde_json::Map<String, Value>, Vec<(u64, String)>, Vec<String>), String> {
    let mut p = serde_json::Map::new();
    let path = curve_ids(inputs, 1);
    let path_sketch = skeleton_sketch(app, groups, 1, &path, not).ok_or("the skeleton lines must be in one sketch")?;
    let mut lines: Vec<(u64, String)> = path.iter().map(|l| (path_sketch, l.clone())).collect();
    let mut keys = path.clone();
    let mut more = Vec::new();
    for (i, row) in groups.rows.iter().enumerate() {
        // A group with no lines yet (just added) is left out, so the rest still previews.
        let ids = curve_ids(inputs, i + 2);
        if ids.is_empty() {
            continue;
        }
        let sketch = skeleton_sketch(app, groups, i + 2, &ids, not)
            .ok_or_else(|| format!("the lines of {} must be in one sketch", tr(GROUP_LABELS.get(i).copied().unwrap_or(""))))?;
        let mut g = json!({"path_sketch": sketch, "path": ids});
        if !row.angle.trim().is_empty() {
            g["angle"] = json!(row.angle);
        }
        if let Some(corner) = ["mitre", "butt"].get(row.corner.wrapping_sub(1)) {
            g["corner"] = json!(corner);
        }
        let (locate, mirror) = row.place_params();
        if let Some(l) = locate {
            g["locate"] = l;
        }
        if let Some(m) = mirror {
            g["mirror"] = m;
        }
        lines.extend(ids.iter().map(|l| (sketch, l.clone())));
        keys.extend(ids.iter().map(|l| solidcraft_engine::doc::weld::member_key(more.len() + 1, l)));
        more.push(g);
    }
    p.insert("path_sketch".into(), json!(path_sketch));
    p.insert("path".into(), json!(path));
    if !more.is_empty() {
        p.insert("groups".into(), Value::Array(more));
    }
    let key = |line: &(u64, String)| lines.iter().position(|l| l == line).and_then(|i| keys.get(i)).cloned();
    let joints: Vec<WeldJoint> = groups
        .joints
        .iter()
        .filter_map(|j| {
            let members = [key(&j.lines[0])?, key(&j.lines[1])?];
            let through = j.through.and_then(|t| members.get(t).cloned());
            Some(WeldJoint { members, corner: j.corner, through })
        })
        .collect();
    p.insert("joints".into(), serde_json::to_value(joints).unwrap_or_default());
    Ok((p, lines, keys))
}

/// Structural Member `editing`'s members as it was last built: each one's skeleton line (sketch,
/// line id) and body name. Where it isn't built (rolled back to edit), the stored names, those
/// past them numbered after the first as the evaluator does. Empty for a new feature (its rows
/// keep the line ids).
fn body_names(app: &SolveApp, editing: Option<u64>) -> Vec<((u64, String), String)> {
    let Some(f) = editing.and_then(|id| app.session.doc.feature(id)) else { return Vec::new() };
    let made = app.session.model.result(f.id);
    let keys = f.kind.weld_keys();
    let base = f.body_names.first().map(|n| n.trim()).filter(|n| !n.is_empty());
    solidcraft_engine::doc::weld::member_lines(&f.kind)
        .into_iter()
        .zip(&keys)
        .enumerate()
        .filter_map(|(i, ((sketch, line), key))| {
            let built = made.and_then(|r| r.state.welds.iter().find(|w| w.feature == f.id && w.line == *key)).map(|w| w.body.clone());
            let stored = || f.body_names.get(i).filter(|n| !n.trim().is_empty()).cloned().or_else(|| base.map(|b| format!("{b} ({})", i + 1)));
            Some(((sketch.0, line.to_string()), built.or_else(stored)?))
        })
        .collect()
}

/// Most corners the Structural Member dialog lists.
const SHOWN_CORNERS: usize = 100;

/// The Corners rows: each corner the skeleton makes (from `weld.corners`), joined as the feature's
/// corner says or otherwise: a mitre, or a butt with either member running through.
fn corner_rows(
    app: &mut SolveApp,
    ui: &mut egui::Ui,
    inputs: &[SelInput],
    groups: &mut Groups,
    corner: usize,
    not: Option<u64>,
    editing: Option<u64>,
) {
    groups.hovered = None;
    let Ok((mut p, lines, keys)) = skeleton(app, inputs, groups, not) else { return };
    if keys.len() < 2 {
        return;
    }
    p.insert("corner".into(), json!(if corner == 1 { "butt" } else { "mitre" }));
    let now = (app.session.revision, Value::Object(p));
    if groups.corners.as_ref().is_none_or(|(k, _)| *k != now) {
        let found = app.session.execute("weld.corners", &now.1).map_err(|e| e.to_string());
        groups.corners = Some((now, found));
    }
    let found = match groups.corners.as_ref().map(|(_, f)| f) {
        Some(Ok(v)) => v["corners"].as_array().cloned().unwrap_or_default(),
        _ => return,
    };
    if found.is_empty() {
        return;
    }
    form_row(ui, "ui.corners", |ui| ui.label(format!("{}", found.len())));
    let mut hovered = None;
    let line = |key: &str| keys.iter().position(|k| k == key).and_then(|i| lines.get(i)).cloned();
    let names = body_names(app, editing);
    let name = |key: &str| line(key).and_then(|l| names.iter().find(|(n, _)| *n == l)).map_or(key.to_string(), |(_, b)| b.clone());
    for (n, c) in found.iter().take(SHOWN_CORNERS).enumerate() {
        let (Some(a), Some(b)) = (c["members"][0].as_str(), c["members"][1].as_str()) else { continue };
        let (Some(la), Some(lb)) = (line(a), line(b)) else { continue };
        let own = groups.joints.iter().position(|j| j.joins(&la, &lb));
        let mut choice = match own.and_then(|i| groups.joints.get(i)) {
            None => 0,
            Some(j) if j.corner == WeldCorner::Mitre => 1,
            Some(j) => match j.through.and_then(|t| j.lines.get(t)) {
                Some(t) if *t == lb => 3,
                _ => 2,
            },
        };
        let (na, nb) = (name(a), name(b));
        let through = |m: &str| crate::i18n::format("weld.butt_through", &[("member", m)]);
        let labels = [tr("Same as feature").to_string(), tr("Mitre").to_string(), through(&na), through(&nb)];
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let before = choice;
        let full = labels.get(choice).copied().unwrap_or_default();
        let top = ui.cursor().top();
        form_row(ui, &format!("{na} · {nb}"), |ui| ui_kit::dropdown(ui, &format!("weld_corner_{n}"), &mut choice, &labels).on_hover_text(full));
        let row = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
        if ui.rect_contains_pointer(row)
            && let Some(at) = c["at"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
            && let [x, y, z] = at[..]
        {
            hovered = Some(Vec3::new(x, y, z));
        }
        if choice != before {
            if let Some(i) = own {
                groups.joints.remove(i);
            }
            let joint = |corner, through| LineJoint { lines: [la.clone(), lb.clone()], corner, through };
            match choice {
                1 => groups.joints.push(joint(WeldCorner::Mitre, None)),
                2 => groups.joints.push(joint(WeldCorner::Butt, Some(0))),
                3 => groups.joints.push(joint(WeldCorner::Butt, Some(1))),
                _ => {}
            }
        }
    }
    if found.len() > SHOWN_CORNERS {
        ui_kit::text(ui, &format!("… {} more corners", found.len() - SHOWN_CORNERS), ui_kit::Text::Note("weld_corners"));
    }
    groups.hovered = hovered;
}
