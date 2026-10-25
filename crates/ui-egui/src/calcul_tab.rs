//! The Calcul tab: the ribbon only holds buttons to choose from. Two windows do the work:
//! the variable database (document → groups → elements) and the formula editor, which picks
//! the element, lists its variables, operators and functions, previews the result and inserts
//! it into the text as an equation. The engine side is `var.*` and `calc.*`
//! (`engine/src/cmd/vars.rs`).

use egui::{ScrollArea, TextEdit, Ui};
use serde_json::{Value, json};

use crate::WordApp;
use crate::equation_tab::{math_big_button, math_menu_button};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::group;

/// What the tab remembers between frames (kept in egui's memory, not in the document).
#[derive(Clone, Default)]
struct State {
    /// The element the calculation is about, e.g. `RDC/D2` (empty = the document).
    path: String,
    formula: String,
    unit: String,
    /// Window flags.
    show_vars: bool,
    show_formula: bool,
    /// The level shown in the variable window.
    level: String,
    /// New variable form.
    new_name: String,
    new_value: String,
    new_unit: String,
    /// A path typed to start a new group or element.
    new_path: String,
    message: String,
    error: bool,
}

fn state_id() -> egui::Id {
    egui::Id::unique("calcul_tab_state")
}

fn load(ui: &Ui) -> State {
    ui.data(|d| d.get_temp::<State>(state_id())).unwrap_or_default()
}

fn store(ui: &Ui, st: State) {
    ui.data_mut(|d| d.insert_temp(state_id(), st));
}

/// Every level path in the tree (ancestors included), the document first.
fn known_paths(app: &mut WordApp) -> Vec<String> {
    let mut out = vec![String::new()];
    if let Ok(v) = app.session.run("var.list", &json!({})) {
        for row in v.get("variables").and_then(Value::as_array).into_iter().flatten() {
            if let Some(p) = row.get("path").and_then(Value::as_str) {
                let mut acc = String::new();
                for seg in p.split('/').filter(|s| !s.is_empty()) {
                    if !acc.is_empty() {
                        acc.push('/');
                    }
                    acc.push_str(seg);
                    if !out.contains(&acc) {
                        out.push(acc.clone());
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn path_label(p: &str) -> String {
    if p.trim().is_empty() { "Document".to_string() } else { p.to_string() }
}

/// What each function category shows as its picture.
fn category_picture(cat: &str) -> &'static str {
    match cat {
        "Nombres" => "√(x)",
        "Trigonométrie" => "sin⁡θ",
        "Exponentielle et log" => "e^x",
        "Algèbre" => "x²−1=0",
        "Analyse" => "∫_(−x)^x",
        "Matrices" => "[■(1&0@0&1)]",
        _ => "x",
    }
}

pub fn show(app: &mut WordApp, ui: &mut Ui) {
    let mut st = load(ui);

    database_group(app, ui, &mut st);
    formulas_group(app, ui, &mut st);
    symbols_group(app, ui, &mut st);
    structures_group(app, ui, &mut st);
    functions_group(app, ui, &mut st);

    variables_window(app, ui.ctx(), &mut st);
    formula_window(app, ui.ctx(), &mut st);

    store(ui, st);
}

/// Base de données: open the variable window, pick the element.
fn database_group(app: &mut WordApp, ui: &mut Ui, st: &mut State) {
    group(ui, "Base de données", None, app, |ui, app| {
        if math_big_button(ui, app, "x=5", "Variables").on_hover_text("Ouvrir la base de variables du document").clicked() {
            st.show_vars = true;
            st.level = st.path.clone();
        }
        let paths = known_paths(app);
        let label = format!("Élément\n{}", path_label(&st.path));
        math_menu_button(ui, app, "x_i", &label, |ui, _| {
            ui.set_min_width(200.0);
            ui.label(egui::RichText::new("Élément du calcul").font(semibold(12.0)));
            for p in paths {
                if ui.selectable_label(p == st.path, path_label(&p)).clicked() {
                    st.path = p;
                    ui.close();
                }
            }
        });
    });
}

/// Formules: open the editor, calculate.
fn formulas_group(app: &mut WordApp, ui: &mut Ui, st: &mut State) {
    group(ui, "Formules", None, app, |ui, app| {
        if math_big_button(ui, app, "ƒ(x)", "Éditeur de\nformule").on_hover_text("Écrire une formule, choisir les variables et les fonctions").clicked() {
            st.show_formula = true;
        }
        if math_big_button(ui, app, "=", "Calculer").on_hover_text("Calculer la formule et insérer le résultat dans le texte").clicked() {
            if st.formula.trim().is_empty() {
                st.show_formula = true;
            } else {
                run_compute(app, st, true);
            }
        }
        if math_big_button(ui, app, "M:=", "Calculer\nle texte").on_hover_text("Transforme les formules tapées dans le texte (M := p*L^2/8 [kN·m]) en résultats qui se mettent à jour tout seuls").clicked() {
            let r = app.run("calc.scan", json!({"path": st.path}));
            set_message(st, r.map(|v| format!("{} formule(s) calculée(s)", v.get("calculated").and_then(Value::as_u64).unwrap_or(0))));
        }
        if math_big_button(ui, app, "↻", "Recalculer").on_hover_text("Met à jour toutes les formules du document").clicked() {
            let r = app.run("calc.refresh", json!({}));
            set_message(st, r.map(|v| format!("{} formule(s) mise(s) à jour", v.get("updated").and_then(Value::as_u64).unwrap_or(0))));
        }
        if !st.message.is_empty() {
            let t = Tokens::get(ui.ctx());
            let color = if st.error { ui.visuals().error_fg_color } else { t.accent };
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.set_max_width(190.0);
                ui.label(egui::RichText::new(&st.message).font(regular(11.5)).color(color));
            });
        }
    });
}

/// Symbols: a small grid, like the equation tab's.
fn symbols_group(app: &mut WordApp, ui: &mut Ui, st: &mut State) {
    const SYMS: [(&str, &str); 20] = [
        ("±", "+-"), ("∞", "inf"), ("=", "="), ("≠", "!="), ("×", "*"), ("÷", "/"), ("^", "^"), ("√", "sqrt(▯)"), ("π", "pi"), ("°", "*pi/180"),
        ("<", "<"), (">", ">"), ("≤", "<="), ("≥", ">="), ("(", "("), (")", ")"), (";", ";"), ("%", "/100"), ("e", "e"), ("∑", "sum(▯; i; 1; n)"),
    ];
    group(ui, "Symboles", None, app, |ui, _| {
        egui::Grid::new("calcul_symbols").spacing([2.0, 2.0]).show(ui, |ui| {
            for (i, (shown, tpl)) in SYMS.iter().enumerate() {
                if ui.add_sized([26.0, 24.0], egui::Button::new(*shown)).clicked() {
                    add_to_formula(st, tpl);
                    st.show_formula = true;
                }
                if i % 10 == 9 {
                    ui.end_row();
                }
            }
        });
    });
}

/// Structures: big buttons (fraction, power, radical…) that write into the formula.
fn structures_group(app: &mut WordApp, ui: &mut Ui, st: &mut State) {
    const STRUCTS: [(&str, &str, &str); 8] = [
        ("x/y", "Fraction", "(▯)/(▯)"),
        ("e^x", "Exposant", "(▯)^(▯)"),
        ("√x", "Radical", "sqrt(▯)"),
        ("∫", "Intégrale", "integrate(▯; x)"),
        ("Σ", "Somme", "sum(▯; i; 1; n)"),
        ("(▯)", "Parenthèses", "(▯)"),
        ("lim", "Limite", "limit(▯; x; 0)"),
        ("[1 0|0 1]", "Matrice", "[[▯; ▯]; [▯; ▯]]"),
    ];
    group(ui, "Structures", None, app, |ui, app| {
        for (pic, label, tpl) in STRUCTS {
            if math_big_button(ui, app, pic, label).clicked() {
                add_to_formula(st, tpl);
                st.show_formula = true;
            }
        }
    });
}

/// Functions: one big picture button per category; a choice writes into the formula.
fn functions_group(app: &mut WordApp, ui: &mut Ui, st: &mut State) {
    let fns: Vec<Value> = app.session.run("calc.functions", &json!({})).ok().and_then(|v| v.get("functions").and_then(Value::as_array).cloned()).unwrap_or_default();
    let mut cats: Vec<String> = Vec::new();
    for f in &fns {
        let c = f.get("category").and_then(Value::as_str).unwrap_or("").to_string();
        if !cats.contains(&c) {
            cats.push(c);
        }
    }
    group(ui, "Fonctions", None, app, |ui, app| {
        for cat in &cats {
            let label = cat.replacen(" et ", "\net ", 1);
            math_menu_button(ui, app, category_picture(cat), &label, |ui, _| {
                ui.set_min_width(300.0);
                for f in fns.iter().filter(|f| f.get("category").and_then(Value::as_str) == Some(cat.as_str())) {
                    let name = f.get("name").and_then(Value::as_str).unwrap_or("");
                    let tpl = f.get("template").and_then(Value::as_str).unwrap_or("");
                    let help = f.get("help").and_then(Value::as_str).unwrap_or("");
                    let ok = f.get("available").and_then(Value::as_bool).unwrap_or(false);
                    let tip = if ok { help.to_string() } else { format!("{help} (moteur de calcul formel : bientôt)") };
                    if ui.add_enabled(ok, egui::Button::new(format!("{name}    {tpl}"))).on_hover_text(tip).clicked() {
                        add_to_formula(st, tpl);
                        st.show_formula = true;
                        ui.close();
                    }
                }
            });
        }
    });
}

/// Adds a piece to the formula. A template with a blank `▯` wraps what is already written after
/// `:=` (so "solve" or "diff" applies to the formula being typed); otherwise it is appended.
fn add_to_formula(st: &mut State, piece: &str) {
    if st.formula.trim().is_empty() {
        st.formula = if piece.contains('▯') { format!("r := {piece}") } else { format!("r := {piece}") };
        return;
    }
    if piece.contains('▯') {
        let (head, expr) = match st.formula.split_once(":=") {
            Some((h, e)) => (format!("{}:= ", h.trim_end()), e.trim().to_string()),
            None => (String::new(), st.formula.trim().to_string()),
        };
        if !expr.is_empty() {
            st.formula = format!("{head}{}", piece.replacen('▯', &expr, 1));
            return;
        }
        st.formula = format!("{head}{piece}");
        return;
    }
    st.formula.push_str(piece);
}

fn run_compute(app: &mut WordApp, st: &mut State, insert: bool) {
    let r = app.run("calc.compute", json!({"path": st.path, "formula": st.formula, "unit": st.unit, "insert": insert}));
    let unit = st.unit.clone();
    set_message(st, r.map(|v| format!("= {} {}", v.get("text").and_then(Value::as_str).unwrap_or(""), unit)));
    if insert {
        app.canvas.want_focus = true;
    }
}

fn set_message(st: &mut State, r: Result<String, String>) {
    match r {
        Ok(m) => {
            st.message = m;
            st.error = false;
        }
        Err(e) => {
            st.message = e;
            st.error = true;
        }
    }
}

/// The variable database: levels on the left, the selected level's variables on the right.
fn variables_window(app: &mut WordApp, ctx: &egui::Context, st: &mut State) {
    let mut open = st.show_vars;
    if !open {
        return;
    }
    egui::Window::new("Base de variables").id(egui::Id::new("calcul_vars_window")).open(&mut open).default_width(640.0).default_height(380.0).show(ctx, |ui| {
        let rows: Vec<Value> = app.session.run("var.list", &json!({})).ok().and_then(|v| v.get("variables").and_then(Value::as_array).cloned()).unwrap_or_default();
        let mut paths = known_paths(app);
        let typed = st.new_path.trim().trim_matches('/').to_string();
        if !typed.is_empty() && !paths.contains(&typed) {
            paths.push(typed);
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(190.0);
                ui.label(egui::RichText::new("Niveaux").font(semibold(12.0)));
                ScrollArea::vertical().id_salt("levels").max_height(250.0).show(ui, |ui| {
                    for p in &paths {
                        let depth = p.split('/').filter(|s| !s.is_empty()).count();
                        ui.horizontal(|ui| {
                            ui.add_space(12.0 * depth as f32);
                            let name = if p.is_empty() { "Document".to_string() } else { p.rsplit('/').next().unwrap_or(p).to_string() };
                            if ui.selectable_label(*p == st.level, name).clicked() {
                                st.level = p.clone();
                            }
                        });
                    }
                });
                ui.separator();
                ui.label("Nouveau groupe ou élément");
                ui.add(TextEdit::singleline(&mut st.new_path).hint_text("RDC/D2").desired_width(180.0));
                if ui.button("Ouvrir ce niveau").clicked() {
                    st.level = st.new_path.trim().trim_matches('/').to_string();
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("Variables de : {}", path_label(&st.level))).font(semibold(12.0)));
                ui.weak("Chaque niveau hérite du niveau au-dessus ; une valeur saisie ici remplace celle du dessus.");
                egui::Grid::new("vars_grid").num_columns(5).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.strong("Nom");
                    ui.strong("Valeur ou =formule");
                    ui.strong("Unité");
                    ui.strong("Résultat");
                    ui.label("");
                    ui.end_row();
                    let level = st.level.clone();
                    let mine: Vec<&Value> = rows.iter().filter(|r| r.get("path").and_then(Value::as_str) == Some(level.as_str())).collect();
                    for row in mine {
                        let name = row.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let value = row.get("value").and_then(Value::as_str).unwrap_or("").to_string();
                        let unit = row.get("unit").and_then(Value::as_str).unwrap_or("").to_string();
                        ui.label(&name);
                        let id = ui.id().with(("val", &level, &name));
                        let mut buf = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| value.clone());
                        let r = ui.add(TextEdit::singleline(&mut buf).desired_width(150.0));
                        ui.data_mut(|d| d.insert_temp(id, buf.clone()));
                        if r.lost_focus() && buf != value {
                            let res = app.run("var.set", json!({"path": level, "name": name, "value": buf, "unit": unit}));
                            set_message(st, res.map(|_| format!("{name} modifié")));
                        }
                        if !r.has_focus() {
                            ui.data_mut(|d| d.remove_temp::<String>(id));
                        }
                        ui.label(&unit);
                        let res = app.session.run("var.resolve", &json!({"path": level, "name": name})).ok();
                        ui.label(res.as_ref().and_then(|v| v.get("text")).and_then(Value::as_str).unwrap_or("?"));
                        if ui.small_button("✕").on_hover_text("Supprimer cette variable").clicked() {
                            let _ = app.run("var.delete", json!({"path": level, "name": name}));
                        }
                        ui.end_row();
                    }
                    // The new-variable row.
                    ui.add(TextEdit::singleline(&mut st.new_name).hint_text("nom").desired_width(60.0));
                    ui.add(TextEdit::singleline(&mut st.new_value).hint_text("valeur ou =formule").desired_width(150.0));
                    ui.add(TextEdit::singleline(&mut st.new_unit).hint_text("unité").desired_width(50.0));
                    ui.label("");
                    if ui.button("Ajouter").clicked() {
                        let res = app.run("var.set", json!({"path": level, "name": st.new_name, "value": st.new_value, "unit": st.new_unit}));
                        let done = format!("{} ajouté", st.new_name);
                        let ok = res.is_ok();
                        set_message(st, res.map(|_| done));
                        if ok {
                            st.new_name.clear();
                            st.new_value.clear();
                            st.new_unit.clear();
                        }
                    }
                    ui.end_row();
                });
                if !st.message.is_empty() {
                    let color = if st.error { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
                    ui.colored_label(color, &st.message);
                }
            });
        });
    });
    st.show_vars = open;
}

/// The formula editor: pick the element, write with the help of buttons, preview, insert.
fn formula_window(app: &mut WordApp, ctx: &egui::Context, st: &mut State) {
    let mut open = st.show_formula;
    if !open {
        return;
    }
    egui::Window::new("Éditeur de formule").id(egui::Id::new("calcul_formula_window")).open(&mut open).default_width(520.0).show(ctx, |ui| {
        let paths = known_paths(app);
        ui.horizontal(|ui| {
            ui.label("Élément :");
            egui::ComboBox::from_id_salt("calcul_formula_element").selected_text(path_label(&st.path)).show_ui(ui, |ui| {
                for p in paths {
                    let label = path_label(&p);
                    ui.selectable_value(&mut st.path, p, label);
                }
            });
            ui.label("Unité du résultat :");
            ui.add(TextEdit::singleline(&mut st.unit).desired_width(60.0).hint_text("kN·m"));
        });
        ui.add_space(4.0);
        ui.label("Formule  (nom := expression)");
        ui.add(TextEdit::multiline(&mut st.formula).font(regular(15.0)).desired_rows(2).desired_width(f32::INFINITY).hint_text("M := p*L^2/8"));

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Opérateurs").font(semibold(12.0)));
        ui.horizontal_wrapped(|ui| {
            for op in [":=", "+", "−", "×", "÷", "^", "(", ")", ";", "π"] {
                if ui.button(op).clicked() {
                    let piece = match op {
                        "−" => "-",
                        "×" => "*",
                        "÷" => "/",
                        "π" => "pi",
                        o => o,
                    };
                    st.formula.push_str(piece);
                }
            }
            if ui.button("Effacer").clicked() {
                st.formula.clear();
            }
        });

        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("Variables de « {} »", path_label(&st.path))).font(semibold(12.0)));
        let vars: Vec<Value> = app.session.run("var.list", &json!({"path": st.path})).ok().and_then(|v| v.get("variables").and_then(Value::as_array).cloned()).unwrap_or_default();
        if vars.is_empty() {
            ui.weak("Aucune variable : ouvre la base de variables pour en créer.");
        }
        ui.horizontal_wrapped(|ui| {
            for v in &vars {
                let name = v.get("name").and_then(Value::as_str).unwrap_or("");
                let result = v.get("result").and_then(Value::as_str).unwrap_or("?");
                let unit = v.get("unit").and_then(Value::as_str).unwrap_or("");
                if ui.button(name).on_hover_text(format!("{name} = {result} {unit}")).clicked() {
                    st.formula.push_str(name);
                }
            }
        });

        // Live preview: the part after `:=` is evaluated without defining anything.
        ui.add_space(6.0);
        let expr = st.formula.split_once(":=").map_or(st.formula.as_str(), |(_, e)| e).trim().to_string();
        if !expr.is_empty() {
            match app.session.run("var.eval", &json!({"path": st.path, "formula": expr})) {
                Ok(v) => {
                    ui.label(egui::RichText::new(format!("Aperçu : {} {}", v.get("text").and_then(Value::as_str).unwrap_or(""), st.unit)).font(semibold(14.0)));
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                }
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Insérer dans le document").on_hover_text("Écrit le résultat dans le texte, comme une équation").clicked() {
                run_compute(app, st, true);
            }
            if ui.button("Enregistrer sans insérer").on_hover_text("Garde la variable calculée sans rien écrire dans le texte").clicked() {
                run_compute(app, st, false);
            }
        });
        if !st.message.is_empty() {
            let color = if st.error { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
            ui.colored_label(color, &st.message);
        }
    });
    st.show_formula = open;
}

/// Right-click on a calculated formula: choose its element and see its source, from a list.
pub fn formula_context(app: &mut WordApp, ui: &mut Ui) {
    let Ok(info) = app.session.run("calc.formulaAt", &json!({})) else { return };
    let path = info.get("path").and_then(Value::as_str).unwrap_or("").to_string();
    let source = info.get("source").and_then(Value::as_str).unwrap_or("").to_string();
    ui.label(egui::RichText::new(format!("Formule : {source}")).font(semibold(12.0)));
    ui.menu_button("Élément du calcul", |ui| {
        for p in known_paths(app) {
            if ui.selectable_label(p == path, path_label(&p)).clicked() {
                let _ = app.run("calc.setFormula", json!({"path": p}));
                ui.close();
            }
        }
    });
    ui.menu_button("Variables disponibles", |ui| {
        let vars: Vec<Value> = app.session.run("var.list", &json!({"path": path})).ok().and_then(|v| v.get("variables").and_then(Value::as_array).cloned()).unwrap_or_default();
        for v in vars {
            let name = v.get("name").and_then(Value::as_str).unwrap_or("");
            let result = v.get("result").and_then(Value::as_str).unwrap_or("?");
            let unit = v.get("unit").and_then(Value::as_str).unwrap_or("");
            ui.label(format!("{name} = {result} {unit}"));
        }
    });
    if ui.button("Recalculer").clicked() {
        let _ = app.run("calc.refresh", json!({}));
        ui.close();
    }
    ui.separator();
}
