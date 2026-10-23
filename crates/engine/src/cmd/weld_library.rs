//! The weldment profile library: profiles drawn once, kept in a file, and picked by name for
//! Structural Members. A member copies the profile it uses into the design, so the design builds
//! without the library and a later change to the library changes no design.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_doc::WeldStock;
use solidcraft_doc::metadata::MAX_KEYS;
use solidcraft_doc::weld::{centred, check_lengths, check_stock};

use super::CommandSpec;
use super::bom::property;
use super::features::{profiles, sketch_id};
use crate::params::{bad, bool_, str_};
use crate::{Result, Session};

/// What the library file says it is.
const SCHEMA: &str = "solidcraft-weld-profiles/1";
/// Most profiles one library holds.
pub const MAX_PROFILES: usize = 2000;
/// Largest library file read.
const MAX_BYTES: usize = 16 << 20;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("weld.profile_save", "Save Profile to Library", save).icon("export").noundo().params(
        "name (as the cut list will name the stock, e.g. `SHS 50×50×3`); sketch; profiles? (one closed region, holes included; \
         default: the sketch's only profile); description?: text (the cut list description of its members; default the name); \
         properties?: {name: text, …} (free properties its members carry to the cut list: standard, grade, supplier…); \
         lengths?: [lengths the stock is bought in, e.g. [\"20 ft\", \"40 ft\"]: bare numbers in the design's unit] (the cut list \
         counts the bars to buy from them); library? (the library file; default the per-user one); replace?: bool (default false: a profile of that name already \
         there is an error) → the saved profile's summary. The profile is kept with its centroid at the origin; the design is \
         not changed",
    ),
    CommandSpec::new("weld.profiles", "Profile Library", list).icon("inspect").noundo().doc_only().params(
        "query? (words a profile's name must all contain, any case); type? (only profiles of this type); library? → {library, \
         types: [{type, count}] (of the profiles matching `query`), profiles: [{name, type, size, width_mm, height_mm, area_mm2, \
         holes, description, properties, lengths_mm}], total (in the library)}. A profile's type is its name up to the first number (`HSS`, `L`, `IPE`; `Other` for a \
         name without one), its size the rest; profiles are listed by type, then by the numbers in their size. A library file \
         not made yet lists no profiles",
    ),
    CommandSpec::new("weld.profile_get", "Library Profile", get)
        .icon("inspect")
        .noundo()
        .doc_only()
        .params("name; library? → {library, profile: its summary, stock: {name, region}} (what weld.member's `stock` takes, and stores)"),
    CommandSpec::new("weld.profile_delete", "Delete Library Profile", delete)
        .icon("delete")
        .noundo()
        .doc_only()
        .params("name; library? → removes the profile from the library. Designs whose members use it keep their copy"),
    CommandSpec::new("weld.profile_lengths", "Library Profile Stock Lengths", set_lengths).icon("settings").noundo().doc_only().params(
        "name; lengths: [lengths the stock is bought in: bare numbers in the design's unit] (empty or null: none); library? → the \
         profile's summary. Members already made keep the lengths they copied (weld.stock_lengths sets a design's own)",
    ),
];

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Library {
    schema: String,
    profiles: Vec<WeldStock>,
}

/// The per-user library file: `SOLIDCRAFT_WELD_PROFILES`, else `weld-profiles.json` in the
/// per-user data folder. Tests never touch the user's real library.
pub fn default_path() -> Option<String> {
    if cfg!(test) {
        return Some(
            solidcraft_io::vfs::temp_dir().join(format!("solidcraft-test-weld-profiles-{}.json", std::process::id())).to_string_lossy().into(),
        );
    }
    if let Some(p) = solidcraft_geom::env::var("WELD_PROFILES").filter(|p| !p.is_empty()) {
        return Some(p);
    }
    crate::recovery::data_dir().map(|d| d.join("weld-profiles.json").to_string_lossy().into())
}

/// The library file a command names (`library`), or the per-user one.
fn library_path(p: &Value, cmd: &str) -> Result<String> {
    let path = match p.get("library").filter(|v| !v.is_null()) {
        Some(v) => v.as_str().ok_or_else(|| bad(cmd, "`library` must be a file path"))?.to_string(),
        None => default_path().ok_or_else(|| bad(cmd, "no per-user data folder here: give `library`"))?,
    };
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(bad(cmd, "invalid library path"));
    }
    solidcraft_io::vfs::check(&path).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(path)
}

fn read(path: &str, cmd: &str) -> Result<Library> {
    if !solidcraft_io::vfs::exists(path) {
        return Ok(Library { schema: SCHEMA.into(), profiles: Vec::new() });
    }
    let bytes = solidcraft_io::vfs::read(path, MAX_BYTES).map_err(|e| bad(cmd, format!("could not read the profile library: {e}")))?;
    let lib: Library = serde_json::from_slice(&bytes).map_err(|e| bad(cmd, format!("not a profile library: {e}")))?;
    if lib.schema != SCHEMA {
        return Err(bad(cmd, format!("profile library schema `{}` is not `{SCHEMA}`", lib.schema)));
    }
    if lib.profiles.len() > MAX_PROFILES {
        return Err(bad(cmd, format!("a profile library holds at most {MAX_PROFILES} profiles")));
    }
    Ok(lib)
}

fn write(path: &str, lib: &Library, cmd: &str) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(lib).map_err(|e| bad(cmd, e.to_string()))?;
    if bytes.len() > MAX_BYTES {
        return Err(bad(cmd, "the profile library would be too large"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let result = {
        let p = std::path::Path::new(path);
        match p.parent().filter(|d| !d.as_os_str().is_empty()) {
            Some(d) => std::fs::create_dir_all(d),
            None => Ok(()),
        }
        .and_then(|()| solidcraft_io::write_atomic(p, &bytes, false))
    };
    #[cfg(target_arch = "wasm32")]
    let result = solidcraft_io::vfs::write(path, &bytes);
    result.map_err(|e| bad(cmd, format!("could not save the profile library: {e}")))
}

/// Names are matched without regard to case or surrounding space.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The free properties `p` gives under `properties` (empty ones left out), checked as a
/// component's are.
pub(super) fn properties(p: &Value, cmd: &str) -> Result<BTreeMap<String, String>> {
    let given = match p.get("properties") {
        None | Some(Value::Null) => return Ok(BTreeMap::new()),
        Some(Value::Object(v)) if v.len() <= MAX_KEYS => v,
        Some(Value::Object(_)) => return Err(bad(cmd, format!("`properties` can hold at most {MAX_KEYS} names"))),
        Some(_) => return Err(bad(cmd, "`properties` must be an object: {name: text, …}")),
    };
    let mut out = BTreeMap::new();
    for (name, value) in given {
        if let Some(text) = property(name, value, cmd)? {
            out.insert(name.clone(), text);
        }
    }
    Ok(out)
}

/// A profile as the library lists it: its name, extent, area, hole count, and the cut list
/// description and properties it gives its members.
fn summary(stock: &WeldStock) -> Value {
    let pts = stock.region.outer.polyline(1e-2);
    let span = |f: fn(&solidcraft_geom::Vec2) -> f64| {
        let (lo, hi) = pts.iter().map(f).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if hi >= lo { hi - lo } else { 0.0 }
    };
    let (kind, size) = type_and_size(&stock.name);
    json!({
        "name": stock.name,
        "type": kind,
        "size": size,
        "width_mm": span(|p| p.x),
        "height_mm": span(|p| p.y),
        "area_mm2": stock.region.area(),
        "holes": stock.region.holes.len(),
        "description": stock.description,
        "properties": stock.properties,
        "lengths_mm": stock.lengths,
    })
}

/// A profile's type and size, read from its name: the type is the text before the first number
/// (`HSS`, `L`, `IPE`), the size the rest; a name without a number is of type `Other`.
fn type_and_size(name: &str) -> (String, String) {
    let name = name.trim();
    match name.find(|c: char| c.is_ascii_digit()).map(|i| name.split_at(i)) {
        Some((kind, size)) if !kind.trim().is_empty() => (kind.trim().to_string(), size.trim().to_string()),
        _ => ("Other".into(), name.to_string()),
    }
}

/// The numbers in a size, in order, to sort sizes by: `1-1/2x1-1/2x1/8` → 1.5, 1.5, 0.125;
/// `Pipe 1-1/4 Sch 40` → 1.25, 40.
fn size_numbers(size: &str) -> Vec<f64> {
    let value = |t: &str| match t.split_once('/') {
        Some((n, d)) => n.parse::<f64>().ok().zip(d.parse::<f64>().ok()).filter(|(_, d)| *d != 0.0).map(|(n, d)| n / d),
        None => t.parse::<f64>().ok(),
    };
    let mut out = Vec::new();
    let mut rest = size;
    while let Some(i) = rest.find(|c: char| c.is_ascii_digit()) {
        let (_, from) = rest.split_at(i);
        let end = from.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '/' | '-'))).unwrap_or(from.len());
        let (token, tail) = from.split_at(end);
        rest = tail;
        let token = token.trim_end_matches('-');
        let v = match token.split_once('-') {
            Some((whole, part)) => value(whole).zip(value(part)).map(|(w, f)| w + f),
            None => value(token),
        };
        out.push(v.unwrap_or(0.0));
    }
    out
}

/// The library profile named `name`, checked, as a member copies it.
pub(super) fn find(p: &Value, name: &str, cmd: &str) -> Result<WeldStock> {
    let path = library_path(p, cmd)?;
    let lib = read(&path, cmd)?;
    let stock = lib.profiles.into_iter().find(|s| same_name(&s.name, name)).ok_or_else(|| bad(cmd, format!("no profile `{name}` in the library")))?;
    check_stock(&stock.name, &stock.region).map_err(|e| bad(cmd, e))?;
    check_lengths(&stock.lengths).map_err(|e| bad(cmd, format!("library profile `{name}`: {e}")))?;
    Ok(stock)
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.profile_save";
    let name = str_(p, "name").map(str::trim).unwrap_or_default().to_string();
    let sketch = sketch_id(s, p.get("sketch"), cmd, "sketch")?;
    let sel = profiles(p, cmd)?;
    let replace = match p.get("replace") {
        None | Some(Value::Null) => false,
        Some(_) => bool_(p, "replace").ok_or_else(|| bad(cmd, "`replace` must be true or false"))?,
    };
    let st = s.model.state();
    let ss = st.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
    let regions = solidcraft_doc::select_profiles(ss, &sel).map_err(|e| bad(cmd, e.to_string()))?;
    let region = match regions.as_slice() {
        [one] => centred(one),
        [] => return Err(bad(cmd, "the sketch has no closed profile")),
        _ => return Err(bad(cmd, "pick one closed profile (one region, holes included)")),
    };
    let description = match p.get("description") {
        None | Some(Value::Null) => String::new(),
        Some(v) => property("description", v, cmd)?.unwrap_or_default(),
    };
    let properties = properties(p, cmd)?;
    let lengths = super::weld::stock_lengths(s, p, "lengths", cmd)?;
    let stock = WeldStock { name, region, description, properties, lengths };
    check_stock(&stock.name, &stock.region).map_err(|e| bad(cmd, e))?;
    let path = library_path(p, cmd)?;
    let mut lib = read(&path, cmd)?;
    match lib.profiles.iter().position(|o| same_name(&o.name, &stock.name)) {
        Some(_) if !replace => return Err(bad(cmd, format!("the library already has a profile `{}` (set `replace` to replace it)", stock.name))),
        Some(i) => lib.profiles.remove(i),
        None if lib.profiles.len() >= MAX_PROFILES => return Err(bad(cmd, format!("the library is full ({MAX_PROFILES} profiles)"))),
        None => stock.clone(),
    };
    let out = summary(&stock);
    lib.profiles.push(stock);
    lib.profiles.sort_by_key(|o| o.name.to_lowercase());
    lib.schema = SCHEMA.into();
    write(&path, &lib, cmd)?;
    Ok(json!({"library": path, "profile": out}))
}

fn list(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.profiles";
    let path = library_path(p, cmd)?;
    let lib = read(&path, cmd)?;
    let words: Vec<String> = str_(p, "query").unwrap_or_default().split_whitespace().map(str::to_lowercase).collect();
    let only = str_(p, "type").map(str::trim).filter(|t| !t.is_empty());
    let mut found: Vec<(String, Vec<f64>, &WeldStock)> = lib
        .profiles
        .iter()
        .filter(|o| {
            let n = o.name.to_lowercase();
            words.iter().all(|w| n.contains(w.as_str()))
        })
        .map(|o| {
            let (kind, size) = type_and_size(&o.name);
            (kind, size_numbers(&size), o)
        })
        .collect();
    found.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.2.name.cmp(&b.2.name))
    });
    let mut types: Vec<(String, usize)> = Vec::new();
    for (kind, _, _) in &found {
        match types.last_mut() {
            Some((t, n)) if t.eq_ignore_ascii_case(kind) => *n += 1,
            _ => types.push((kind.clone(), 1)),
        }
    }
    let shown: Vec<Value> =
        found.iter().filter(|(kind, _, _)| only.is_none_or(|t| kind.eq_ignore_ascii_case(t))).map(|(_, _, o)| summary(o)).collect();
    let types: Vec<Value> = types.into_iter().map(|(t, n)| json!({"type": t, "count": n})).collect();
    Ok(json!({"library": path, "types": types, "profiles": shown, "total": lib.profiles.len()}))
}

fn get(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.profile_get";
    let name = str_(p, "name").ok_or_else(|| bad(cmd, "`name` must name a library profile"))?;
    let stock = find(p, name, cmd)?;
    Ok(json!({"library": library_path(p, cmd)?, "profile": summary(&stock), "stock": stock}))
}

fn delete(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.profile_delete";
    let name = str_(p, "name").ok_or_else(|| bad(cmd, "`name` must name a library profile"))?;
    let path = library_path(p, cmd)?;
    let mut lib = read(&path, cmd)?;
    let i = lib.profiles.iter().position(|o| same_name(&o.name, name)).ok_or_else(|| bad(cmd, format!("no profile `{name}` in the library")))?;
    let gone = lib.profiles.remove(i);
    write(&path, &lib, cmd)?;
    Ok(json!({"library": path, "deleted": gone.name}))
}

fn set_lengths(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "weld.profile_lengths";
    let name = str_(p, "name").ok_or_else(|| bad(cmd, "`name` must name a library profile"))?;
    if p.get("lengths").is_none() {
        return Err(bad(cmd, "`lengths` is required (empty: none)"));
    }
    let lengths = super::weld::stock_lengths(s, p, "lengths", cmd)?;
    let path = library_path(p, cmd)?;
    let mut lib = read(&path, cmd)?;
    let stock = lib.profiles.iter_mut().find(|o| same_name(&o.name, name)).ok_or_else(|| bad(cmd, format!("no profile `{name}` in the library")))?;
    stock.lengths = lengths;
    let out = summary(stock);
    write(&path, &lib, cmd)?;
    Ok(json!({"library": path, "profile": out}))
}
