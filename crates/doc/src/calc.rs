//! Document variables and a small formula evaluator (the "Calcul" feature).
//!
//! Variables live in a tree: the document (path `""`), groups (`"RDC"`) and elements
//! (`"RDC/D2"`). A lookup starts at the element and walks up to the document, so a group can
//! hold a default that an element overrides. A value is a number, text, or a formula that starts
//! with `=` (evaluated at the element being asked about).
//!
//! The whole store is kept in one custom document property ([`PROP_NAME`]) as JSON, so it
//! round-trips through DOCX without touching the writer.
//!
//! Never panics: all limits are checked, recursion is bounded, input is treated as hostile.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Custom document property that holds the serialized [`VarStore`].
pub const PROP_NAME: &str = "wordcraft.vars";

const MAX_FORMULA: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_NAME: usize = 128;
const MAX_PATH: usize = 512;
const MAX_VARS: usize = 20_000;

/// One variable at one level of the tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Var {
    /// A number (`"5.2"`, `"5,2"`), text, or a formula starting with `=`.
    pub value: String,
    pub unit: String,
    pub label: String,
}

/// Where a resolved value came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub number: Option<f64>,
    pub text: String,
    pub unit: String,
    /// The path of the level that defined it.
    pub origin: String,
}

/// The tree of variables: path → name → variable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VarStore {
    pub levels: BTreeMap<String, BTreeMap<String, Var>>,
}

/// Normalise a path: trims, drops empty segments, joins with `/`.
pub fn clean_path(p: &str) -> String {
    p.split('/').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

/// A valid variable name: a letter or `_` first, then letters, digits, `_`. Greek letters and
/// accents are letters, so `σ`, `γ_M0` and `é` work.
pub fn valid_name(n: &str) -> bool {
    let mut it = n.chars();
    let Some(first) = it.next() else { return false };
    n.chars().count() <= MAX_NAME && (first.is_alphabetic() || first == '_') && it.all(|c| c.is_alphanumeric() || c == '_')
}

impl VarStore {
    /// Read the store from the document's custom property; a missing or damaged property gives
    /// an empty store (never an error: a bad file must still open).
    pub fn load(doc: &crate::Document) -> VarStore {
        doc.custom_prop(PROP_NAME).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
    }

    /// Write the store back to the document (removes the property when empty).
    pub fn save(&self, doc: &mut crate::Document) {
        if self.levels.values().all(BTreeMap::is_empty) {
            doc.remove_custom_prop(PROP_NAME);
        } else if let Ok(s) = serde_json::to_string(self) {
            doc.set_custom_prop(PROP_NAME, &s);
        }
    }

    pub fn count(&self) -> usize {
        self.levels.values().map(BTreeMap::len).sum()
    }

    /// Define or replace a variable.
    pub fn set(&mut self, path: &str, name: &str, var: Var) -> Result<(), String> {
        let path = clean_path(path);
        if path.len() > MAX_PATH {
            return Err("path too long".into());
        }
        if !valid_name(name) {
            return Err(format!("`{name}` is not a valid variable name"));
        }
        if var.value.len() > MAX_FORMULA {
            return Err("value too long".into());
        }
        let is_new = !self.levels.get(&path).is_some_and(|l| l.contains_key(name));
        if is_new && self.count() >= MAX_VARS {
            return Err("too many variables".into());
        }
        self.levels.entry(path).or_default().insert(name.to_string(), var);
        Ok(())
    }

    /// Remove a variable; returns whether it existed. Empty levels disappear.
    pub fn delete(&mut self, path: &str, name: &str) -> bool {
        let path = clean_path(path);
        let removed = self.levels.get_mut(&path).is_some_and(|l| l.remove(name).is_some());
        if self.levels.get(&path).is_some_and(BTreeMap::is_empty) {
            self.levels.remove(&path);
        }
        removed
    }

    /// The levels from `path` up to the document, nearest first.
    fn chain(path: &str) -> Vec<String> {
        let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut out = Vec::new();
        loop {
            out.push(parts.join("/"));
            if parts.pop().is_none() {
                break;
            }
        }
        out
    }

    /// Every variable visible from `path` (own, then inherited), nearest definition winning.
    pub fn visible(&self, path: &str) -> BTreeMap<String, (String, Var)> {
        let mut out: BTreeMap<String, (String, Var)> = BTreeMap::new();
        for level in Self::chain(&clean_path(path)) {
            if let Some(vars) = self.levels.get(&level) {
                for (n, v) in vars {
                    out.entry(n.clone()).or_insert_with(|| (level.clone(), v.clone()));
                }
            }
        }
        out
    }

    /// Resolve `name` as seen from `path`. Formulas are evaluated at `path` (the element asked
    /// about), not at the level that defined them, so a group formula adapts to each element.
    pub fn resolve(&self, path: &str, name: &str) -> Result<Resolved, String> {
        self.resolve_at(&clean_path(path), name, 0, &mut Vec::new())
    }

    fn resolve_at(&self, path: &str, name: &str, depth: usize, stack: &mut Vec<String>) -> Result<Resolved, String> {
        if depth > MAX_DEPTH {
            return Err("formulas nest too deeply".into());
        }
        let Some((origin, var)) = Self::chain(path).into_iter().find_map(|lvl| self.levels.get(&lvl).and_then(|l| l.get(name)).map(|v| (lvl, v))) else {
            return Err(format!("unknown variable `{name}`"));
        };
        let key = format!("{path}\u{1}{name}");
        if stack.contains(&key) {
            return Err(format!("`{name}` depends on itself"));
        }
        let raw = var.value.trim();
        if let Some(f) = raw.strip_prefix('=') {
            stack.push(key);
            let r = eval_with(f, &mut |n| self.resolve_at(path, n, depth + 1, stack).and_then(|r| r.number.ok_or_else(|| format!("`{n}` is not a number"))));
            stack.pop();
            let x = r?;
            return Ok(Resolved { number: Some(x), text: format_number(x), unit: var.unit.clone(), origin });
        }
        Ok(Resolved { number: parse_number(raw), text: raw.to_string(), unit: var.unit.clone(), origin })
    }

    /// Evaluate a formula (without a leading `=`) as seen from `path`.
    pub fn eval(&self, path: &str, formula: &str) -> Result<f64, String> {
        let path = clean_path(path);
        let mut stack = Vec::new();
        eval_with(formula, &mut |n| self.resolve_at(&path, n, 1, &mut stack).and_then(|r| r.number.ok_or_else(|| format!("`{n}` is not a number"))))
    }
}

/// Parse a number written with a point or a French decimal comma.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".").replace(['\u{a0}', ' '], "");
    let x: f64 = t.parse().ok()?;
    x.is_finite().then_some(x)
}

/// Format a number for display: at most 6 significant digits, French decimal comma, no trailing
/// zeros.
pub fn format_number(x: f64) -> String {
    if !x.is_finite() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    let digits = 6i32;
    let mag = x.abs().log10().floor() as i32;
    let s = if !(-4..digits + 3).contains(&mag) {
        format!("{x:.*e}", (digits - 1) as usize)
    } else {
        let dec = (digits - 1 - mag).clamp(0, 12) as usize;
        format!("{x:.dec$}")
    };
    let s = if s.contains('e') {
        s
    } else if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    s.replace('.', ",")
}

// ---------------------------------------------------------------- evaluator

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(char),
    /// A comparison: `<` `>` `l` (≤) `g` (≥) `=` `n` (≠).
    Cmp(char),
    LP,
    RP,
    Comma,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    if s.len() > MAX_FORMULA {
        return Err("formula too long".into());
    }
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while let Some(&c) = cs.get(i) {
        match c {
            ' ' | '\t' | '\u{a0}' => i += 1,
            '0'..='9' | '.' | ',' => {
                let st = i;
                while cs.get(i).is_some_and(|c| c.is_ascii_digit() || *c == '.' || *c == ',') {
                    i += 1;
                }
                // exponent: 1e-3
                if cs.get(i).is_some_and(|c| *c == 'e' || *c == 'E') {
                    let mut j = i + 1;
                    if cs.get(j).is_some_and(|c| *c == '+' || *c == '-') {
                        j += 1;
                    }
                    if cs.get(j).is_some_and(char::is_ascii_digit) {
                        while cs.get(j).is_some_and(char::is_ascii_digit) {
                            j += 1;
                        }
                        i = j;
                    }
                }
                let t: String = cs.get(st..i).unwrap_or_default().iter().collect();
                out.push(Tok::Num(parse_number(&t).ok_or_else(|| format!("bad number `{t}`"))?));
            }
            '<' | '>' | '=' | '!' | '≤' | '≥' | '≠' => {
                let next_eq = cs.get(i + 1) == Some(&'=');
                let (code, adv) = match (c, next_eq) {
                    ('<', true) => ('l', 2),
                    ('>', true) => ('g', 2),
                    ('=', true) => ('=', 2),
                    ('!', true) => ('n', 2),
                    ('<', false) if cs.get(i + 1) == Some(&'>') => ('n', 2),
                    ('<', false) => ('<', 1),
                    ('>', false) => ('>', 1),
                    ('=', false) => ('=', 1),
                    ('≤', _) => ('l', 1),
                    ('≥', _) => ('g', 1),
                    ('≠', _) => ('n', 1),
                    _ => return Err("unexpected `!`".into()),
                };
                out.push(Tok::Cmp(code));
                i += adv;
            }
            '(' => {
                out.push(Tok::LP);
                i += 1;
            }
            ')' => {
                out.push(Tok::RP);
                i += 1;
            }
            ';' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '+' | '-' | '/' | '^' | '%' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '*' => {
                if cs.get(i + 1) == Some(&'*') {
                    out.push(Tok::Op('^'));
                    i += 2;
                } else {
                    out.push(Tok::Op('*'));
                    i += 1;
                }
            }
            '·' | '×' | '⋅' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '−' => {
                out.push(Tok::Op('-'));
                i += 1;
            }
            'π' => {
                out.push(Tok::Id("pi".into()));
                i += 1;
            }
            c if c.is_alphabetic() || c == '_' => {
                let st = i;
                while cs.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                    i += 1;
                }
                out.push(Tok::Id(cs.get(st..i).unwrap_or_default().iter().collect()));
            }
            other => return Err(format!("unexpected `{other}`")),
        }
    }
    Ok(out)
}

struct Parser<'a> {
    t: Vec<Tok>,
    i: usize,
    look: &'a mut dyn FnMut(&str) -> Result<f64, String>,
    depth: usize,
    /// Loop variables (innermost last), looked up before the document's variables.
    locals: Vec<(String, f64)>,
    /// Loop iterations spent so far (bounded by `MAX_STEPS`).
    steps: usize,
}

const MAX_STEPS: usize = 200_000;

/// Evaluate `formula`; `look` supplies variable values.
pub fn eval_with(formula: &str, look: &mut dyn FnMut(&str) -> Result<f64, String>) -> Result<f64, String> {
    let t = lex(formula)?;
    if t.is_empty() {
        return Err("empty formula".into());
    }
    let mut p = Parser { t, i: 0, look, depth: 0, locals: Vec::new(), steps: 0 };
    let v = p.or_()?;
    if p.i != p.t.len() {
        return Err("unexpected text after the formula".into());
    }
    if v.is_finite() { Ok(v) } else { Err("the result is not a finite number".into()) }
}

/// Evaluate a formula that uses no variables (only numbers, `pi` and functions).
pub fn eval_const(formula: &str) -> Result<f64, String> {
    eval_with(formula, &mut |n| Err(format!("unknown variable `{n}`")))
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn word(&self) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Id(w)) => match w.as_str() {
                "or" | "ou" => Some("or"),
                "and" | "et" => Some("and"),
                "not" | "non" => Some("not"),
                _ => None,
            },
            _ => None,
        }
    }

    /// `a or b` (1 for true, 0 for false).
    fn or_(&mut self) -> Result<f64, String> {
        let mut a = self.and_()?;
        while self.word() == Some("or") {
            self.i += 1;
            let b = self.and_()?;
            a = f64::from(a != 0.0 || b != 0.0);
        }
        Ok(a)
    }

    fn and_(&mut self) -> Result<f64, String> {
        let mut a = self.not_()?;
        while self.word() == Some("and") {
            self.i += 1;
            let b = self.not_()?;
            a = f64::from(a != 0.0 && b != 0.0);
        }
        Ok(a)
    }

    fn not_(&mut self) -> Result<f64, String> {
        if self.word() == Some("not") {
            self.i += 1;
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err("formula nests too deeply".into());
            }
            let v = self.not_();
            self.depth -= 1;
            return Ok(f64::from(v? == 0.0));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<f64, String> {
        let a = self.sum()?;
        if let Some(Tok::Cmp(c)) = self.peek().cloned() {
            self.i += 1;
            let b = self.sum()?;
            let eq = (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
            return Ok(f64::from(match c {
                '<' => a < b && !eq,
                '>' => a > b && !eq,
                'l' => a < b || eq,
                'g' => a > b || eq,
                '=' => eq,
                _ => !eq,
            }));
        }
        Ok(a)
    }

    /// After a `(`: the token ranges of the arguments up to the matching `)`, which is consumed.
    fn arg_ranges(&mut self) -> Result<Vec<(usize, usize)>, String> {
        let mut out = Vec::new();
        let mut start = self.i;
        let mut level = 0usize;
        while let Some(t) = self.t.get(self.i) {
            match t {
                Tok::LP => level += 1,
                Tok::RP if level == 0 => {
                    out.push((start, self.i));
                    self.i += 1;
                    return Ok(out);
                }
                Tok::RP => level -= 1,
                Tok::Comma if level == 0 => {
                    out.push((start, self.i));
                    start = self.i + 1;
                }
                _ => {}
            }
            self.i += 1;
            if out.len() > 16 {
                return Err("too many arguments".into());
            }
        }
        Err("missing `)`".into())
    }

    /// Evaluate the tokens in `range` (with the current loop variables) without moving `self.i`.
    fn eval_range(&mut self, range: (usize, usize)) -> Result<f64, String> {
        let back = self.i;
        self.i = range.0;
        let r = self.or_();
        let ok = self.i == range.1;
        self.i = back;
        let v = r?;
        if ok { Ok(v) } else { Err("unexpected text in the argument".into()) }
    }

    fn range_name(&self, range: (usize, usize)) -> Result<String, String> {
        match (self.t.get(range.0), range.1.saturating_sub(range.0)) {
            (Some(Tok::Id(n)), 1) => Ok(n.clone()),
            _ => Err("a variable name is expected here".into()),
        }
    }

    fn step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS { Err("loop runs too long".into()) } else { Ok(()) }
    }

    /// `if(c; a; b)`, `sum(e; i; a; b)`, `prod(e; i; a; b)`, `while(x; x0; cond; next)`.
    /// Only the chosen branch is evaluated.
    fn lazy_form(&mut self, name: &str) -> Result<Option<f64>, String> {
        let kind = match name {
            "if" | "si" => 0,
            "sum" | "somme" => 1,
            "prod" | "produit" => 2,
            "while" | "tantque" => 3,
            _ => return Ok(None),
        };
        self.i += 1; // the `(`
        let r = self.arg_ranges()?;
        let after = self.i;
        let v = match (kind, r.as_slice()) {
            (0, [c, a, b]) => {
                let cond = self.eval_range(*c)?;
                self.eval_range(if cond != 0.0 { *a } else { *b })?
            }
            (1 | 2, [e, i, lo, hi]) => {
                let var = self.range_name(*i)?;
                let (lo, hi) = (self.eval_range(*lo)?.round(), self.eval_range(*hi)?.round());
                let mut acc = if kind == 1 { 0.0 } else { 1.0 };
                let mut k = lo;
                self.locals.push((var, 0.0));
                while k <= hi {
                    self.step()?;
                    if let Some(l) = self.locals.last_mut() {
                        l.1 = k;
                    }
                    match self.eval_range(*e) {
                        Ok(x) => acc = if kind == 1 { acc + x } else { acc * x },
                        Err(err) => {
                            self.locals.pop();
                            return Err(err);
                        }
                    }
                    k += 1.0;
                }
                self.locals.pop();
                acc
            }
            (3, [x, init, cond, next]) => {
                let var = self.range_name(*x)?;
                let start = self.eval_range(*init)?;
                self.locals.push((var, start));
                let out = loop {
                    if let Err(e) = self.step() {
                        self.locals.pop();
                        return Err(e);
                    }
                    match self.eval_range(*cond) {
                        Ok(c) if c == 0.0 => break Ok(self.locals.last().map_or(start, |l| l.1)),
                        Ok(_) => {}
                        Err(e) => break Err(e),
                    }
                    match self.eval_range(*next) {
                        Ok(n) => {
                            if let Some(l) = self.locals.last_mut() {
                                l.1 = n;
                            }
                        }
                        Err(e) => break Err(e),
                    }
                };
                self.locals.pop();
                out?
            }
            _ => return Err(format!("wrong arguments for `{name}`")),
        };
        self.i = after;
        Ok(Some(v))
    }

    fn sum(&mut self) -> Result<f64, String> {
        let mut a = self.product()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.i += 1;
            let b = self.product()?;
            a = if c == '+' { a + b } else { a - b };
        }
        Ok(a)
    }

    fn product(&mut self) -> Result<f64, String> {
        let mut a = self.unary()?;
        while let Some(Tok::Op(c @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.i += 1;
            let b = self.unary()?;
            a = match c {
                '*' => a * b,
                '/' => {
                    if b == 0.0 {
                        return Err("division by zero".into());
                    }
                    a / b
                }
                _ => {
                    if b == 0.0 {
                        return Err("division by zero".into());
                    }
                    a % b
                }
            };
        }
        Ok(a)
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.i += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.i += 1;
            let e = self.unary()?; // right-associative
            if e.abs() > 400.0 {
                return Err("exponent too large".into());
            }
            return Ok(base.powf(e));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<f64, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("formula nests too deeply".into());
        }
        let r = self.atom_inner();
        self.depth -= 1;
        r
    }

    fn atom_inner(&mut self) -> Result<f64, String> {
        let Some(t) = self.peek().cloned() else { return Err("the formula ends too early".into()) };
        self.i += 1;
        match t {
            Tok::Num(x) => Ok(x),
            Tok::LP => {
                let v = self.or_()?;
                self.expect_rp()?;
                Ok(v)
            }
            Tok::Id(name) => {
                if self.peek() == Some(&Tok::LP)
                    && let Some(v) = self.lazy_form(&name)?
                {
                    return Ok(v);
                }
                if self.peek() == Some(&Tok::LP) {
                    self.i += 1;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::RP) {
                        self.i += 1;
                    } else {
                        loop {
                            args.push(self.or_()?);
                            match self.peek() {
                                Some(Tok::Comma) => self.i += 1,
                                Some(Tok::RP) => {
                                    self.i += 1;
                                    break;
                                }
                                _ => return Err("expected `)` or `;`".into()),
                            }
                            if args.len() > 64 {
                                return Err("too many arguments".into());
                            }
                        }
                    }
                    return call(&name, &args);
                }
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    "e_" => Ok(std::f64::consts::E),
                    "true" | "vrai" => Ok(1.0),
                    "false" | "faux" => Ok(0.0),
                    _ => match self.locals.iter().rev().find(|(n, _)| *n == name) {
                        Some((_, v)) => Ok(*v),
                        None => (self.look)(&name),
                    },
                }
            }
            _ => Err("unexpected symbol".into()),
        }
    }

    fn expect_rp(&mut self) -> Result<(), String> {
        if self.peek() == Some(&Tok::RP) {
            self.i += 1;
            Ok(())
        } else {
            Err("missing `)`".into())
        }
    }
}

fn call(name: &str, a: &[f64]) -> Result<f64, String> {
    let one = |f: fn(f64) -> f64| -> Result<f64, String> {
        match a {
            [x] => Ok(f(*x)),
            _ => Err(format!("`{name}` takes one argument")),
        }
    };
    match name {
        "sqrt" | "racine" => match a {
            [x] if *x >= 0.0 => Ok(x.sqrt()),
            [_] => Err("square root of a negative number".into()),
            _ => Err("`sqrt` takes one argument".into()),
        },
        "abs" => one(f64::abs),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "exp" => one(f64::exp),
        "ln" => one(f64::ln),
        "log" | "log10" => one(f64::log10),
        "ceil" | "plafond" => one(f64::ceil),
        "floor" | "plancher" => one(f64::floor),
        "round" | "arrondi" => match a {
            [x] => Ok(x.round()),
            [x, n] if n.abs() <= 12.0 => {
                let k = 10f64.powi(*n as i32);
                Ok((x * k).round() / k)
            }
            _ => Err("`round` takes a number and optional digits".into()),
        },
        "min" | "max" => {
            let Some(&first) = a.first() else { return Err(format!("`{name}` needs arguments")) };
            Ok(a.iter().copied().fold(first, |m, x| if name == "min" { m.min(x) } else { m.max(x) }))
        }
        _ => Err(format!("unknown function `{name}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_and_loops() {
        let e = eval_const;
        assert_eq!(e("if(3 > 2; 10; 20)").unwrap(), 10.0);
        assert_eq!(e("if(3 <= 2 or 1 = 1; 1; 0)").unwrap(), 1.0);
        assert_eq!(e("si(not (2 > 1); 1; 5/1)").unwrap(), 5.0);
        assert_eq!(e("if(1 > 0; 7; 1/0)").unwrap(), 7.0, "untaken branch is not evaluated");
        assert_eq!(e("sum(i^2; i; 1; 4)").unwrap(), 30.0);
        assert_eq!(e("prod(i; i; 1; 5)").unwrap(), 120.0);
        assert_eq!(e("while(n; 1; n < 100; n*2)").unwrap(), 128.0);
        assert!(e("while(n; 1; n > 0; n+1)").is_err(), "endless loop is cut");
        assert!(e("if(1; 2)").is_err());
    }

    fn store() -> VarStore {
        let mut s = VarStore::default();
        let v = |x: &str| Var { value: x.into(), ..Var::default() };
        s.set("", "fck", v("35")).unwrap();
        s.set("RDC", "h", v("200")).unwrap();
        s.set("RDC", "e", v("0,7")).unwrap();
        s.set("RDC/D1", "L", v("5")).unwrap();
        s.set("RDC/D2", "L", v("6,2")).unwrap();
        s.set("RDC/D2", "h", v("250")).unwrap();
        s.set("RDC", "p", v("=h*e")).unwrap();
        s.set("RDC/D1", "M", v("=p*L^2/8")).unwrap();
        s
    }

    #[test]
    fn arithmetic() {
        assert_eq!(eval_const("1+2*3").unwrap(), 7.0);
        assert_eq!(eval_const("2^3^2").unwrap(), 512.0);
        assert_eq!(eval_const("-2^2").unwrap(), -4.0);
        assert_eq!(eval_const("(1+2)·3").unwrap(), 9.0);
        assert!((eval_const("sqrt(16)+max(1;5;3)").unwrap() - 9.0).abs() < 1e-12);
        assert!((eval_const("2,5*2").unwrap() - 5.0).abs() < 1e-12);
    }

    #[test]
    fn inheritance_and_override() {
        let s = store();
        assert_eq!(s.resolve("RDC/D1", "h").unwrap().number, Some(200.0));
        assert_eq!(s.resolve("RDC/D1", "h").unwrap().origin, "RDC");
        assert_eq!(s.resolve("RDC/D2", "h").unwrap().number, Some(250.0));
        assert_eq!(s.resolve("RDC/D2", "fck").unwrap().origin, "");
        assert!(s.resolve("RDC/D1", "nope").is_err());
    }

    #[test]
    fn group_formula_adapts_to_each_element() {
        let s = store();
        assert!((s.resolve("RDC/D1", "p").unwrap().number.unwrap() - 140.0).abs() < 1e-9);
        assert!((s.resolve("RDC/D2", "p").unwrap().number.unwrap() - 175.0).abs() < 1e-9);
        // M = p L^2 / 8 for D1: 140 * 25 / 8
        assert!((s.resolve("RDC/D1", "M").unwrap().number.unwrap() - 437.5).abs() < 1e-9);
    }

    #[test]
    fn cycles_and_hostile_input_are_errors() {
        let mut s = VarStore::default();
        s.set("", "a", Var { value: "=b+1".into(), ..Var::default() }).unwrap();
        s.set("", "b", Var { value: "=a+1".into(), ..Var::default() }).unwrap();
        assert!(s.resolve("", "a").is_err());
        assert!(eval_const(&"(".repeat(10_000)).is_err());
        assert!(eval_const("1/0").is_err());
        assert!(eval_const("9^9^9").is_err());
        assert!(eval_const("").is_err());
        assert!(eval_const("2 $ 3").is_err());
        assert!(s.set("", "1bad", Var::default()).is_err());
    }

    #[test]
    fn round_trips_through_the_document() {
        let s = store();
        let mut d = crate::Document::new();
        s.save(&mut d);
        assert_eq!(VarStore::load(&d), s);
        let mut e = VarStore::load(&d);
        assert!(e.delete("RDC/D1", "M"));
        assert!(!e.delete("RDC/D1", "M"));
        e.save(&mut d);
        assert!(VarStore::load(&d).resolve("RDC/D1", "M").is_err());
    }

    #[test]
    fn number_format_is_french() {
        assert_eq!(format_number(12.4), "12,4");
        assert_eq!(format_number(437.5), "437,5");
        assert_eq!(format_number(1.0 / 3.0), "0,333333");
        assert_eq!(format_number(2.0), "2");
    }
}
