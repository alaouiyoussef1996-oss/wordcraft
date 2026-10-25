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
        CommandSpec::new("calc.compute", "Calculate", "Math › Calcul", compute).params(
            r#"{"path"?: string, "formula": string ("M := p*L^2/8" defines M; "p*L^2/8" only shows the result), "unit"?: string, "insert"?: bool (default true: put the result in the text as an equation), "display"?: bool}"#,
        ),
        CommandSpec::new("calc.functions", "Calculation Functions", "Math › Calcul", |_, _| Ok(functions_json())).pure().params("{}"),
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

/// The calculation bar's "=": evaluates `name := formula` (or a bare formula) as seen from the
/// element `path`, keeps the definition in the variable tree, and writes `name = result unit` into
/// the text as an equation.
fn compute(s: &mut Session, v: &Value) -> CmdResult {
    let formula = p::req_str(v, "formula")?;
    let unit = p::str(v, "unit").unwrap_or("").trim().to_string();
    let (target, expr) = match formula.split_once(":=") {
        Some((n, e)) => (Some(n.trim().to_string()), e.trim().to_string()),
        None => (None, formula.trim().trim_start_matches('=').to_string()),
    };
    if expr.is_empty() {
        return Err(CmdError::Params("the formula is empty".into()));
    }
    let mut store = VarStore::load(&s.doc);
    let x = store.eval(path(v), &expr).map_err(CmdError::Failed)?;
    let text = format_number(x);
    if let Some(name) = &target {
        store.set(path(v), name, Var { value: format!("={expr}"), unit: unit.clone(), ..Var::default() }).map_err(CmdError::Params)?;
        store.save(&mut s.doc);
    }
    if p::bool(v, "insert").unwrap_or(true) {
        let lhs = target.clone().unwrap_or_else(|| expr.clone());
        let tail = if unit.is_empty() { String::new() } else { format!(" {unit}") };
        super::equation::insert_linear(s, &format!("{lhs}={text}{tail}"), p::bool(v, "display"))?;
    }
    Ok(json!({"name": target, "number": x, "text": text, "unit": unit}))
}

/// One function of the Calcul gallery: name, category, template to insert (`▯` is a blank to
/// fill), a one-line description, and whether the engine can run it today.
pub struct CalcFn {
    pub name: &'static str,
    pub category: &'static str,
    pub template: &'static str,
    pub help: &'static str,
    pub available: bool,
}

const fn f(name: &'static str, category: &'static str, template: &'static str, help: &'static str, available: bool) -> CalcFn {
    CalcFn { name, category, template, help, available }
}

/// The gallery. `available: false` entries need the symbolic engine (planned) and are shown greyed.
pub const CALC_FUNCTIONS: &[CalcFn] = &[
    f("sqrt", "Nombres", "sqrt(▯)", "Racine carrée", true),
    f("abs", "Nombres", "abs(▯)", "Valeur absolue", true),
    f("round", "Nombres", "round(▯; 2)", "Arrondi à n décimales", true),
    f("ceil", "Nombres", "ceil(▯)", "Entier supérieur", true),
    f("floor", "Nombres", "floor(▯)", "Entier inférieur", true),
    f("min", "Nombres", "min(▯; ▯)", "Plus petite valeur", true),
    f("max", "Nombres", "max(▯; ▯)", "Plus grande valeur", true),
    f("sin", "Trigonométrie", "sin(▯)", "Sinus (radians)", true),
    f("cos", "Trigonométrie", "cos(▯)", "Cosinus (radians)", true),
    f("tan", "Trigonométrie", "tan(▯)", "Tangente (radians)", true),
    f("asin", "Trigonométrie", "asin(▯)", "Arc sinus", true),
    f("acos", "Trigonométrie", "acos(▯)", "Arc cosinus", true),
    f("atan", "Trigonométrie", "atan(▯)", "Arc tangente", true),
    f("exp", "Exponentielle et log", "exp(▯)", "Exponentielle", true),
    f("ln", "Exponentielle et log", "ln(▯)", "Logarithme népérien", true),
    f("log", "Exponentielle et log", "log(▯)", "Logarithme décimal", true),
    f("solve", "Algèbre", "solve(▯ = 0; x)", "Résoudre une équation", false),
    f("simplify", "Algèbre", "simplify(▯)", "Simplifier une expression", false),
    f("expand", "Algèbre", "expand(▯)", "Développer", false),
    f("factor", "Algèbre", "factor(▯)", "Factoriser", false),
    f("diff", "Analyse", "diff(▯; x)", "Dériver par rapport à x", false),
    f("integrate", "Analyse", "integrate(▯; x)", "Primitive (ou intégrale avec bornes)", false),
    f("limit", "Analyse", "limit(▯; x; 0)", "Limite", false),
    f("sum", "Analyse", "sum(▯; i; 1; n)", "Somme", false),
    f("det", "Matrices", "det(▯)", "Déterminant", false),
    f("inv", "Matrices", "inv(▯)", "Inverse d'une matrice", false),
    f("transpose", "Matrices", "transpose(▯)", "Transposée", false),
    f("eigenvals", "Matrices", "eigenvals(▯)", "Valeurs propres", false),
    f("linsolve", "Matrices", "linsolve(▯; ▯)", "Résoudre un système A·x = b", false),
];

fn functions_json() -> Value {
    json!({"functions": CALC_FUNCTIONS.iter().map(|c| json!({"name": c.name, "category": c.category, "template": c.template, "help": c.help, "available": c.available})).collect::<Vec<_>>()})
}
