//! Math › Variables: the document's variable tree (document → groups → elements) and the
//! formula evaluator behind the calculation bar. Stored in the document (see
//! `wordcraft_doc::calc`), so it saves with the file and undoes like any edit.

use serde_json::{Value, json};
use wordcraft_doc::calc::{Var, VarStore, format_number};

use crate::{CmdError, CmdResult, CommandSpec, Session, p};

const LOC: &str = "Math › Variables";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("var.set", "Set Variable", LOC, set)
            .params(r#"{"path"?: string ("RDC/D2", default the document), "name": string, "value": string (number, text or "=formula"), "unit"?: string, "label"?: string}"#),
        CommandSpec::new("var.delete", "Delete Variable", LOC, delete).params(r#"{"path"?: string, "name": string}"#),
        CommandSpec::new("var.list", "List Variables", LOC, list)
            .params(r#"{"path"?: string: every variable visible from this element, with where it comes from; omit for the whole tree}"#)
            .pure(),
        CommandSpec::new("var.resolve", "Resolve Variable", LOC, resolve).params(r#"{"path"?: string, "name": string}"#).pure(),
        CommandSpec::new("var.eval", "Evaluate Formula", LOC, eval)
            .params(r#"{"path"?: string, "formula": string (e.g. "p*L^2/8"), "name"?: string: also store the result under this name at `path`}"#),
    ]
}

fn path(v: &Value) -> &str {
    p::str(v, "path").unwrap_or("")
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?;
    let value = match v.get("value") {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(CmdError::Params("`value` (string or number) is required".into())),
    };
    let var = Var { value, unit: p::str(v, "unit").unwrap_or("").to_string(), label: p::str(v, "label").unwrap_or("").to_string() };
    let mut store = VarStore::load(&s.doc);
    store.set(path(v), name, var).map_err(CmdError::Params)?;
    store.save(&mut s.doc);
    describe(&store, path(v), name)
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?;
    let mut store = VarStore::load(&s.doc);
    if !store.delete(path(v), name) {
        return Err(CmdError::Failed(format!("no variable `{name}` at `{}`", path(v))));
    }
    store.save(&mut s.doc);
    Ok(json!({"deleted": name}))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let store = VarStore::load(&s.doc);
    let rows = match p::str(v, "path") {
        Some(pth) => store
            .visible(pth)
            .into_iter()
            .map(|(name, (origin, var))| {
                let r = store.resolve(pth, &name);
                json!({"name": name, "origin": origin, "value": var.value, "unit": var.unit, "label": var.label,
                       "result": r.as_ref().ok().map(|r| r.text.clone()), "error": r.err()})
            })
            .collect::<Vec<_>>(),
        None => store
            .levels
            .iter()
            .flat_map(|(lvl, vars)| vars.iter().map(move |(n, var)| json!({"path": lvl, "name": n, "value": var.value, "unit": var.unit, "label": var.label})))
            .collect(),
    };
    Ok(json!({"count": store.count(), "variables": rows}))
}

fn resolve(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?;
    let store = VarStore::load(&s.doc);
    describe(&store, path(v), name)
}

fn describe(store: &VarStore, path: &str, name: &str) -> CmdResult {
    let r = store.resolve(path, name).map_err(CmdError::Failed)?;
    Ok(json!({"name": name, "path": path, "origin": r.origin, "number": r.number, "text": r.text, "unit": r.unit}))
}

fn eval(s: &mut Session, v: &Value) -> CmdResult {
    let formula = p::req_str(v, "formula")?;
    // `M := p*L^2/8` and `M = ...` define a name; a bare formula only computes.
    let (target, expr) = match formula.split_once(":=") {
        Some((n, e)) => (Some(n.trim().to_string()), e.trim().to_string()),
        None => (p::str(v, "name").map(str::to_string), formula.trim().trim_start_matches('=').to_string()),
    };
    let mut store = VarStore::load(&s.doc);
    let x = store.eval(path(v), &expr).map_err(CmdError::Failed)?;
    let text = format_number(x);
    if let Some(name) = target {
        // Stored as the formula, so it follows the variables it depends on.
        store.set(path(v), &name, Var { value: format!("={expr}"), ..Var::default() }).map_err(CmdError::Params)?;
        store.save(&mut s.doc);
    }
    Ok(json!({"number": x, "text": text}))
}
