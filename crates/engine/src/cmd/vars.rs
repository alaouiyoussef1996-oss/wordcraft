//! Math › Variables: the document's variable tree (document → groups → elements) and the
//! formula evaluator behind the calculation bar. Stored in the document (see
//! `wordcraft_doc::calc`), so it saves with the file and undoes like any edit.

use serde_json::{Value, json};
use wordcraft_doc::calc::{Var, VarStore, format_number};
use wordcraft_doc::InlineObject;
use wordcraft_doc::math::{Math, parse_linear, to_linear};

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
        CommandSpec::new("calc.refresh", "Update Formulas", "Math › Calcul", |s, _| Ok(json!({"updated": refresh(s)}))).params("{}"),
        CommandSpec::new("calc.formulaAt", "Formula at Caret", "Math › Calcul", formula_at).pure().params("{}"),
        CommandSpec::new("calc.setFormula", "Edit Formula at Caret", "Math › Calcul", set_formula)
            .params(r#"{"path"?: string: the element the formula applies to, "source"?: string, "unit"?: string}"#),
        CommandSpec::new("calc.scan", "Calculate Text Formulas", "Math › Calcul", scan)
            .params(r#"{"path"?: string: element used until a line "@RDC/D2" changes it}"#),
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
    refresh(s);
    describe(&store, path(v), name)
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?;
    let mut store = VarStore::load(&s.doc);
    if !store.delete(path(v), name) {
        return Err(CmdError::Failed(format!("no variable `{name}` at `{}`", path(v))));
    }
    store.save(&mut s.doc);
    refresh(s);
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
        refresh(s);
    }
    Ok(json!({"number": x, "text": text}))
}

/// The calculation bar's "=": evaluates `name := formula` (or a bare formula) as seen from the
/// element `path`, keeps the definition in the variable tree, and writes `name = result unit` into
/// the text as an equation.
fn compute(s: &mut Session, v: &Value) -> CmdResult {
    let formula = p::req_str(v, "formula")?;
    let unit = p::str(v, "unit").unwrap_or("").trim().to_string();
    let (target, expr) = split_def(formula);
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
        let shown = to_linear(&parse_linear(&format!("{lhs}={text}{tail}")));
        super::equation::insert_linear(s, &shown, p::bool(v, "display"))?;
        let mut placed = load_placed(s);
        placed.push(Placed { path: path(v).to_string(), source: formula.trim().to_string(), unit: unit.clone(), shown });
        save_placed(s, &placed);
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

/// A formula written into the text: it keeps its source and where it applies, so it can be
/// recomputed when the variables change.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Placed {
    path: String,
    source: String,
    unit: String,
    /// The equation text currently in the document (how the equation is found again).
    shown: String,
}

const PLACED_PROP: &str = "wordcraft.formulas";

fn load_placed(s: &Session) -> Vec<Placed> {
    s.doc.custom_prop(PLACED_PROP).and_then(|x| serde_json::from_str(x).ok()).unwrap_or_default()
}

fn save_placed(s: &mut Session, list: &[Placed]) {
    if list.is_empty() {
        s.doc.remove_custom_prop(PLACED_PROP);
    } else if let Ok(x) = serde_json::to_string(list) {
        s.doc.set_custom_prop(PLACED_PROP, &x);
    }
}

/// `name = result unit` for a formula as seen from its element, or the error.
fn render_placed(store: &mut VarStore, f: &Placed) -> Result<String, String> {
    let (name, expr) = split_def(&f.source);
    let lhs = name.unwrap_or_else(|| expr.clone());
    let x = store.eval(&f.path, &expr)?;
    let tail = if f.unit.is_empty() { String::new() } else { format!(" {}", f.unit) };
    Ok(to_linear(&parse_linear(&format!("{lhs}={}{tail}", format_number(x)))))
}

/// Recomputes every formula written in the text against the current variables. Returns how many
/// equations changed.
fn refresh(s: &mut Session) -> usize {
    let mut list = load_placed(s);
    if list.is_empty() {
        return 0;
    }
    let mut store = VarStore::load(&s.doc);
    let shown: Vec<String> = list.iter().map(|f| f.shown.clone()).collect();
    let mut claimed = vec![false; list.len()];
    let mut changed = 0usize;
    let wants = |o: &InlineObject| matches!(o, InlineObject::Equation { linear, .. } if shown.iter().any(|x| x == linear));
    s.doc.objects_mut(&wants, &mut |o| {
        let InlineObject::Equation { linear, math, .. } = o else { return };
        let Some(i) = list.iter().enumerate().position(|(i, f)| !claimed[i] && f.shown == *linear) else { return };
        claimed[i] = true;
        if let Some(f) = list.get_mut(i)
            && let Ok(new) = render_placed(&mut store, f)
        {
            if new != *linear {
                changed += 1;
            }
            *linear = new.clone();
            *math = Math { nodes: parse_linear(&new), ..Default::default() };
            f.shown = new;
        }
    });
    // Formulas whose equation was deleted from the text are forgotten.
    let kept: Vec<Placed> = list.into_iter().zip(claimed).filter(|(_, c)| *c).map(|(f, _)| f).collect();
    save_placed(s, &kept);
    changed
}

/// Splits `M := p*L^2/8` (or `M = p*L^2/8`) into the name and the expression; a bare formula has no name.
fn split_def(formula: &str) -> (Option<String>, String) {
    if let Some((n, e)) = formula.split_once(":=") {
        return (Some(n.trim().to_string()), e.trim().to_string());
    }
    let t = formula.trim().trim_start_matches('=');
    if let Some((n, e)) = t.split_once('=') {
        let n = n.trim();
        if !n.is_empty() && !e.trim().is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return (Some(n.to_string()), e.trim().trim_start_matches('=').trim().to_string());
        }
    }
    (None, t.trim().to_string())
}

/// Turns the formulas typed in the text into calculated equations. A paragraph that holds only
/// `M := p*L^2/8` or `M := p*L^2/8 [kN·m]` becomes `M = 437,5 kN·m` and keeps its source, so it
/// follows the variables afterwards. A paragraph `@RDC/D2` sets the element for the lines below.
fn scan(s: &mut Session, v: &Value) -> CmdResult {
    let mut current = path(v).to_string();
    let mut done = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < s.doc.body.len().min(100_000) {
        let text = s.doc.para_at(&wordcraft_doc::Pos::body(i, 0)).map(|p| p.text.trim().to_string());
        i += 1;
        let Some(text) = text else { continue };
        if let Some(el) = text.strip_prefix('@') {
            current = el.trim().trim_matches('/').to_string();
            continue;
        }
        if !text.contains(":=") || text.len() > 2000 {
            continue;
        }
        let (formula, unit) = match (text.rfind('['), text.ends_with(']')) {
            (Some(k), true) => (text.get(..k).unwrap_or("").trim().to_string(), text.get(k + 1..text.len() - 1).unwrap_or("").trim().to_string()),
            _ => (text.clone(), String::new()),
        };
        let len = text.chars().count();
        let block = i - 1;
        // The paragraph's own length (leading/trailing blanks included).
        let full = s.doc.para_at(&wordcraft_doc::Pos::body(block, 0)).map_or(len, |p| p.text.chars().count());
        s.sel = crate::Selection { anchor: wordcraft_doc::Pos::body(block, 0), focus: wordcraft_doc::Pos::body(block, full) };
        match compute(s, &json!({"path": current, "formula": formula, "unit": unit, "display": false})) {
            Ok(_) => done += 1,
            Err(e) => errors.push(format!("ligne {}: {e}", block + 1)),
        }
    }
    Ok(json!({"calculated": done, "errors": errors}))
}

/// The index in the registry of the calculated formula under the caret.
fn placed_at_caret(s: &Session) -> Option<(usize, Vec<Placed>)> {
    let at = super::equation::equation_at_caret(s)?;
    let Some(InlineObject::Equation { linear, .. }) = s.doc.para_at(&at).and_then(|p| p.object_at(at.off)) else { return None };
    let list = load_placed(s);
    let i = list.iter().position(|f| f.shown == *linear)?;
    Some((i, list))
}

/// The calculated formula under the caret: its source, element and the variables it can use.
fn formula_at(s: &mut Session, _v: &Value) -> CmdResult {
    let (i, list) = placed_at_caret(s).ok_or_else(|| CmdError::Disabled("no calculated formula at the caret".into()))?;
    let f = list.get(i).ok_or_else(|| CmdError::Failed("formula not found".into()))?;
    Ok(json!({"path": f.path, "source": f.source, "unit": f.unit, "shown": f.shown}))
}

/// Changes the element (and/or source, unit) of the formula under the caret and recomputes it.
fn set_formula(s: &mut Session, v: &Value) -> CmdResult {
    let (i, mut list) = placed_at_caret(s).ok_or_else(|| CmdError::Disabled("no calculated formula at the caret".into()))?;
    if let Some(f) = list.get_mut(i) {
        if let Some(x) = p::str(v, "path") {
            f.path = x.trim().trim_matches('/').to_string();
        }
        if let Some(x) = p::str(v, "source") {
            f.source = x.trim().to_string();
        }
        if let Some(x) = p::str(v, "unit") {
            f.unit = x.trim().to_string();
        }
    }
    save_placed(s, &list);
    let n = refresh(s);
    Ok(json!({"updated": n}))
}
