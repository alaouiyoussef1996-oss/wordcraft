//! The Calcul tab: the document's variable tree (document → groups → elements) and the
//! calculation bar. Write `M := p*L^2/8`, press `=`, and the result goes into the text as an
//! equation. The engine side is `var.*` and `calc.compute` (`engine/src/cmd/vars.rs`).

use egui::{ScrollArea, TextEdit, Ui};
use serde_json::{Value, json};

use crate::WordApp;
use crate::widgets::group;

/// What the tab remembers between frames (kept in egui's memory, not in the document).
#[derive(Clone, Default)]
struct State {
    /// The element the calculation is about, e.g. `RDC/D2` (empty = the document).
    path: String,
    formula: String,
    unit: String,
    var_name: String,
    var_value: String,
    var_unit: String,
    message: String,
    error: bool,
}

fn state_id() -> egui::Id {
    egui::Id::new("calcul_tab_state")
}

fn load(ui: &Ui) -> State {
    ui.data(|d| d.get_temp::<State>(state_id())).unwrap_or_default()
}

fn store(ui: &Ui, st: State) {
    ui.data_mut(|d| d.insert_temp(state_id(), st));
}

/// Every level path in the tree (for the element picker), the document first.
fn known_paths(app: &mut WordApp) -> Vec<String> {
    let mut out = vec![String::new()];
    if let Ok(v) = app.session.run("var.list", &json!({})) {
        for row in v.get("variables").and_then(Value::as_array).into_iter().flatten() {
            if let Some(p) = row.get("path").and_then(Value::as_str) {
                // A group is also needed as a choice when only its children hold variables.
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

pub fn show(app: &mut WordApp, ui: &mut Ui) {
    let mut st = load(ui);

    group(ui, "Variables", None, app, |ui, app| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Élément");
                ui.add(TextEdit::singleline(&mut st.path).desired_width(110.0).hint_text("RDC/D2"));
                let paths = known_paths(app);
                ui.menu_button("▾", |ui| {
                    for p in paths {
                        let label = if p.is_empty() { "(document)".to_string() } else { p.clone() };
                        if ui.button(label).clicked() {
                            st.path = p;
                            ui.close();
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut st.var_name).desired_width(50.0).hint_text("nom"));
                ui.add(TextEdit::singleline(&mut st.var_value).desired_width(70.0).hint_text("valeur ou =formule"));
                ui.add(TextEdit::singleline(&mut st.var_unit).desired_width(40.0).hint_text("unité"));
                if ui.button("Définir").on_hover_text("Créer ou modifier la variable à cet élément").clicked() {
                    let r = app.run("var.set", json!({"path": st.path, "name": st.var_name, "value": st.var_value, "unit": st.var_unit}));
                    set_message(&mut st, r.map(|_| format!("{} défini", st.var_name)));
                }
            });
            ui.menu_button("Voir les variables ▾", |ui| {
                ui.set_min_width(340.0);
                let list = app.session.run("var.list", &json!({"path": st.path})).ok();
                let rows: Vec<Value> = list.as_ref().and_then(|v| v.get("variables")).and_then(Value::as_array).cloned().unwrap_or_default();
                if rows.is_empty() {
                    ui.label("Aucune variable. Définis-en une ci-dessus.");
                }
                ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for row in &rows {
                        let name = row.get("name").and_then(Value::as_str).unwrap_or("");
                        let origin = row.get("origin").and_then(Value::as_str).unwrap_or("");
                        let value = row.get("value").and_then(Value::as_str).unwrap_or("");
                        let unit = row.get("unit").and_then(Value::as_str).unwrap_or("");
                        let result = row.get("result").and_then(Value::as_str).unwrap_or("?");
                        let from = if origin.is_empty() { "document".to_string() } else { origin.to_string() };
                        ui.horizontal(|ui| {
                            if ui.link(format!("{name} = {result} {unit}")).on_hover_text(format!("{value}  (défini dans : {from})")).clicked() {
                                st.var_name = name.to_string();
                                st.var_value = value.to_string();
                                st.var_unit = unit.to_string();
                            }
                            ui.weak(format!("[{from}]"));
                            // Only a variable defined at this very level can be deleted here.
                            if origin == st.path.trim_matches('/') && ui.small_button("✕").on_hover_text("Supprimer").clicked() {
                                let _ = app.run("var.delete", json!({"path": st.path, "name": name}));
                            }
                        });
                    }
                });
            });
        });
    });

    group(ui, "Calcul", None, app, |ui, app| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let edit = ui.add(TextEdit::singleline(&mut st.formula).desired_width(280.0).hint_text("M := p*L^2/8"));
                ui.add(TextEdit::singleline(&mut st.unit).desired_width(50.0).hint_text("unité"));
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let go = ui.add(egui::Button::new("=").min_size(egui::vec2(36.0, 24.0))).on_hover_text("Calculer et insérer le résultat dans le texte").clicked();
                if go || enter {
                    let r = app.run("calc.compute", json!({"path": st.path, "formula": st.formula, "unit": st.unit}));
                    set_message(&mut st, r.map(|v| format!("= {} {}", v.get("text").and_then(Value::as_str).unwrap_or(""), st.unit)));
                    app.canvas.want_focus = true;
                }
            });
            let color = if st.error { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
            ui.colored_label(color, if st.message.is_empty() { "Opérations : + − * / ^  sqrt  min  max  sin  cos  ln  pi" } else { st.message.as_str() });
        });
    });

    store(ui, st);
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
