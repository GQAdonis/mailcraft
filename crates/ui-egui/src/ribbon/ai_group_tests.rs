// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Home tab's AI group (ai-discoverability, SCN-A11Y-001): Ask Mail, Find by Description and
//! AI Settings, there only with a provider service; found by their accessible names and run their
//! commands; keyboard focus rings; command search finds the three; the group never pushes
//! Send/Receive behind `…` at 1024 px in the simplified row; and (G2 a) the search box's text is
//! never sent to a model, while Find by Description sends only what is typed into its own field.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use egui::accesskit::Role;
use egui::{Key, PointerButton, Shape, vec2};
use egui_kittest::kittest::{NodeT, Queryable as _};
use serde_json::{Value, json};

use super::{Mode, behind_more, describe, describe_tab, hit_id, plan, row};
use crate::people_view::testing::{self, Window};
use crate::theme::Tokens;
use crate::{AiProviders, ModelJob};

/// The three, by command: (command id, the ribbon's name for it, its icon).
const AI: [(&str, &str, &str); 3] = [
    ("ui.answers", "Ask Mail", "spark"),
    ("ui.findByDescription", "Find by Description", "findByDescription"),
    ("ui.aiSettings", "AI Settings", "shield"),
];

// ---- A host that writes down everything asked of it ------------------------------------------

/// Everything asked of the AI side of the host: provider calls that do something, model requests
/// (with what they would send), and query embeddings.
#[derive(Clone, Default)]
pub(crate) struct Log(Rc<RefCell<Vec<String>>>);

impl Log {
    fn push(&self, what: String) {
        self.0.borrow_mut().push(what);
    }
    fn all(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}

/// A provider service with one provider on this computer (so nothing waits for a consent), which
/// records every call that does something; reading its settings is not a request.
#[derive(Clone)]
struct Host {
    log: Log,
}

fn sent<T: Send + 'static>(value: T) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(value);
    rx
}

impl AiProviders for Host {
    fn providers(&self) -> Value {
        json!({"active": "local", "providers": [{"id": "local", "kind": "local", "base_url": "http://127.0.0.1:11434/v1", "model": "m"}]})
    }
    fn set_provider(&self, provider: &Value) -> Result<(), String> {
        self.log.push(format!("set_provider {provider}"));
        Ok(())
    }
    fn remove_provider(&self, id: &str) -> Result<(), String> {
        self.log.push(format!("remove_provider {id}"));
        Ok(())
    }
    fn set_active(&self, id: &str) -> Result<(), String> {
        self.log.push(format!("set_active {id}"));
        Ok(())
    }
    fn set_consent(&self, id: &str, account: &str, allowed: bool) -> Result<(), String> {
        self.log.push(format!("set_consent {id} {account} {allowed}"));
        Ok(())
    }
    fn save_key(&self, id: &str, _key: String) -> Receiver<Result<(), String>> {
        self.log.push(format!("save_key {id}"));
        sent(Ok(()))
    }
    fn forget_key(&self, id: &str) -> Receiver<Result<(), String>> {
        self.log.push(format!("forget_key {id}"));
        sent(Ok(()))
    }
    fn has_key(&self, _id: &str) -> Receiver<Result<bool, String>> {
        sent(Ok(false))
    }
    fn test_provider(&self, provider: &Value) -> Receiver<Result<Value, String>> {
        self.log.push(format!("test_provider {provider}"));
        std::sync::mpsc::channel().1
    }
    fn discover_on_device(&self) -> Receiver<Result<Value, String>> {
        self.log.push("discover_on_device".into());
        std::sync::mpsc::channel().1
    }
}

/// The main window on the sample mailbox, `width` wide, with the recording host and its model and
/// embedding services in place.
pub(crate) fn window_with_host(width: f32) -> (Window, Log) {
    let log = Log::default();
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    let services = &mut w.state_mut().services;
    services.ai_providers = Some(Box::new(Host { log: log.clone() }));
    let jobs = log.clone();
    services.model_request = Some(Box::new(move |job: ModelJob| {
        let words: Vec<String> = job.request.messages.iter().map(|m| m.content.clone()).collect();
        jobs.push(format!("model {:?} sources {:?} accounts {:?} :: {}", job.request.kind, job.request.sources, job.accounts, words.join(" | ")));
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(tx);
        rx
    }));
    let embeds = log.clone();
    services.embed_query = Some(Box::new(move |text: String| {
        embeds.push(format!("embed_query {text}"));
        std::sync::mpsc::channel().1
    }));
    w.set_size(vec2(width, 800.0));
    w.run_steps(4);
    (w, log)
}

// ---- The group --------------------------------------------------------------------------------

#[test]
fn the_group_is_there_with_a_provider_service_and_only_then() {
    let (w, _) = window_with_host(1440.0);
    let tab = describe(w.state());
    let group = tab.groups.iter().find(|g| g.title == "AI").expect("an AI group");
    let got: Vec<(&str, &str, String)> = group.items.iter().map(|i| (i.command.as_ref().map_or("", |(c, _)| *c), i.icon, i.name())).collect();
    for (command, name, icon) in AI {
        assert!(got.iter().any(|(c, i, n)| *c == command && *i == icon && n == name), "{command}: {got:?}");
        assert!(crate::icons::NAMES.contains(&icon), "{icon} has a drawing");
    }
    // The last group of Home, after Send/Receive, and nothing but these three.
    assert_eq!(tab.groups.last().map(|g| g.title), Some("AI"));
    assert_eq!(group.items.len(), 3);

    let plain = testing::window("view.mail");
    assert!(plain.state().services.ai_providers.is_none());
    for module in [true, false] {
        let tab = if module { describe(plain.state()) } else { describe_tab(plain.state(), "Home") };
        assert!(tab.groups.iter().all(|g| g.title != "AI"), "no AI group without a provider service");
        assert!(tab.items().all(|i| AI.iter().all(|(c, ..)| i.command.as_ref().is_none_or(|(id, _)| id != c))));
    }
}

#[test]
fn without_a_service_nothing_of_the_group_is_drawn() {
    let mut w = testing::window("view.mail");
    w.set_size(vec2(1440.0, 800.0));
    w.run_steps(4);
    for (command, name, _) in AI {
        assert!(w.state().hit.rect("ribbon", hit_id(command)).is_none(), "{command}");
        // The search box's own Find by Description button is still there (Mail's), the ribbon's isn't.
        if name != "Find by Description" {
            assert!(w.query_by_label(name).is_none(), "{name}");
        }
    }
}

/// The three are found by their accessible names (their labels) in the simplified row and in
/// the classic layout, and each runs its command when clicked.
#[test]
fn scn_a11y_001_the_buttons_have_names_and_run_their_commands() {
    for mode in [Mode::Simplified, Mode::Classic] {
        let (mut w, _) = window_with_host(1440.0);
        w.state_mut().execute("ui.ribbonMode", json!({"value": mode.code()})).unwrap();
        w.run_steps(4);
        for (command, name, _) in AI {
            let rect = w.state().hit.rect("ribbon", hit_id(command)).unwrap_or_else(|| panic!("{mode:?}: {command} is drawn"));
            let named: Vec<_> = w.query_all_by_label(name).filter(|n| n.accesskit_node().role() == Role::Button).collect();
            assert!(
                named.iter().any(|n| n.rect().center().distance(rect.center()) < 1.0 && !n.accesskit_node().is_disabled()),
                "{mode:?}: {name} is a button named so, where the ribbon draws it"
            );
        }
        // Ask Mail opens the Answers pane.
        let ask = w.state().hit.rect("ribbon", hit_id("ui.answers")).unwrap().center();
        testing::click(&mut w, ask, PointerButton::Primary, 1);
        assert!(w.state().ai.answers.is_some(), "{mode:?}: Ask Mail opens Answers");
        // Find by Description opens its panel under the search box.
        let find = w.state().hit.rect("ribbon", hit_id("ui.findByDescription")).unwrap().center();
        testing::click(&mut w, find, PointerButton::Primary, 1);
        assert!(w.state().ai.narrow.is_some(), "{mode:?}: Find by Description opens its panel");
        testing::key(&mut w, Key::Escape);
        // AI Settings opens File › Options at AI assistance.
        let settings = w.state().hit.rect("ribbon", hit_id("ui.aiSettings")).unwrap().center();
        testing::click(&mut w, settings, PointerButton::Primary, 1);
        assert!(w.state().ui.backstage, "{mode:?}: AI Settings opens File › Options");
    }
}

/// Keyboard focus on each of the three draws the focus ring round it (G16 a), and nothing does
/// without it.
#[test]
fn each_button_has_a_focus_ring() {
    for mode in [Mode::Simplified, Mode::Classic] {
        let (mut w, _) = window_with_host(1440.0);
        w.state_mut().execute("ui.ribbonMode", json!({"value": mode.code()})).unwrap();
        w.run_steps(4);
        for (command, name, _) in AI {
            let rect = w.state().hit.rect("ribbon", hit_id(command)).unwrap();
            let ring = |w: &Window| {
                let t = Tokens::get(&w.ctx);
                w.output().shapes.iter().any(|c| {
                    matches!(&c.shape, Shape::Rect(r) if r.stroke.color == t.focus_ring && r.stroke.width >= 2.0 && r.rect.expand(4.0).contains_rect(rect) && rect.expand(4.0).contains_rect(r.rect))
                })
            };
            assert!(!ring(&w), "{mode:?}: {name} has no ring without the keyboard");
            let node = w
                .query_all_by_label(name)
                .find(|n| n.accesskit_node().role() == Role::Button && n.rect().center().distance(rect.center()) < 1.0)
                .unwrap_or_else(|| panic!("{mode:?}: {name}"));
            node.focus();
            w.run_steps(2);
            assert!(ring(&w), "{mode:?}: {name} has a ring with the keyboard");
            w.ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            w.run_steps(2);
        }
    }
}

/// The queries Search Commands is to find the three by: the ribbon's words for each, and the
/// command's own name.
const QUERIES: [(&str, &str); 6] = [
    ("ask mail", "ui.answers"),
    ("answers", "ui.answers"),
    ("find by description", "ui.findByDescription"),
    ("description", "ui.findByDescription"),
    ("AI settings", "ui.aiSettings"),
    ("AI assistance", "ui.aiSettings"),
];

/// Each of the three, under the name and place the window's command list gives it, matches the
/// queries Search Commands is given for it; and the ribbon's names match too.
#[test]
fn the_three_match_their_queries_by_name() {
    for (query, id) in QUERIES {
        let (_, label, location, _) = crate::UI_COMMANDS.iter().find(|c| c.0 == id).copied().unwrap_or_else(|| panic!("{id} is a window command"));
        let matched = crate::dialogs::score(query, tl!(label), &crate::i18n::location(location), id).is_some();
        assert!(matched, "{query:?} matches {id} ({label})");
    }
}

/// Search Commands lists the three (when the window has a provider service) and finds each by
/// the ribbon's words.
#[test]
fn command_search_finds_the_three() {
    let (w, _) = window_with_host(1280.0);
    let all = crate::dialogs::all_commands(w.state());
    let found = |query: &str, id: &str| all.iter().any(|c| c.0 == id && crate::dialogs::score(query, &c.1, &c.2, &c.0).is_some());
    for (query, id) in QUERIES {
        assert!(found(query, id), "{query:?} finds {id}");
    }
}

// ---- The fit at 1024 px ------------------------------------------------------------------------

/// At a 1024-pixel window, in the simplified row, the AI group never puts Send/Receive behind
/// `…`: whatever of the group doesn't fit goes there first. Checked on the plan the row is drawn
/// from, and on what is drawn, with Send/Receive able to run and not.
#[test]
fn at_1024_the_group_does_not_push_send_receive_behind_more() {
    let (mut w, _) = window_with_host(1024.0);
    assert_eq!(w.state().ui.ribbon_mode, Mode::Simplified);
    for can_sync in [false, true] {
        if can_sync {
            // Something to send makes Send/Receive able to run (a large button, with its label).
            w.state_mut().services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
        }
        w.run_steps(4);
        let more = behind_more(&w.ctx);
        assert!(!more.contains(&"ui.sync"), "can sync {can_sync}: Send/Receive is behind …: {more:?}");
        assert!(w.state().hit.rect("ribbon", hit_id("ui.sync")).is_some(), "can sync {can_sync}: Send/Receive is in the row");

        // And the plan agrees: of the AI group and Send/Receive, the group gives way first.
        let tab = describe(w.state());
        let buttons = row(&tab);
        let band = w.state().hit.rect("ribbonBand", 0).expect("the band");
        let sync = buttons.iter().position(|b| b.item.key == "ui.sync").expect("Send/Receive");
        let drawn = |i: usize| buttons.get(i).and_then(|b| w.state().hit.rect("ribbon", hit_id(b.item.key)));
        let width = |i: usize, labelled: bool| match drawn(i) {
            Some(r) if labelled => r.width() + 80.0,
            Some(r) => r.width(),
            None => 40.0,
        };
        for available in [band.width() * 0.6, band.width() * 0.8, band.width()] {
            let p = plan(&buttons, &width, available);
            if p.more.contains(&sync) {
                for (i, b) in buttons.iter().enumerate() {
                    if b.item.section == 6 {
                        assert!(p.more.contains(&i), "{available}: {} stays while Send/Receive goes", b.item.key);
                    }
                }
            }
        }
    }
}

// ---- G2 a: the search box's text never goes to a model ----------------------------------------

/// Typing a sentence in the search box runs the search (and its suggestions) and nothing else: no
/// model request is made or sent, no provider is called, nothing is embedded.
#[test]
fn g2a_typing_in_the_search_box_asks_no_model() {
    let (mut w, log) = window_with_host(1440.0);
    let sentence = "what did Ada say about the quarterly budget last week";
    w.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    w.run_steps(2);
    for word in sentence.split_inclusive(' ') {
        testing::text(&mut w, word);
    }
    testing::key(&mut w, Key::Enter);
    w.run_steps(6);
    assert_eq!(w.state().session.view.search, sentence, "the live filter took it");
    assert_eq!(log.all(), Vec::<String>::new(), "the host was asked nothing");
    assert!(w.state().session.ai.all().is_empty(), "no model request was made");
    assert!(w.state().ai.narrow.is_none() && w.state().ai.answers.is_none(), "no AI place opened by itself");
}

/// With text in the search box, Find by Description (opened from the ribbon) sends what is typed
/// into its own field and nothing of the search box's.
#[test]
fn g2a_find_by_description_sends_only_its_own_field() {
    let (mut w, log) = window_with_host(1440.0);
    let search = "zebra crossing minutes";
    w.state_mut().execute("view.search", json!({"query": search})).unwrap();
    w.run_steps(4);
    let find = w.state().hit.rect("ribbon", hit_id("ui.findByDescription")).expect("Find by Description in the ribbon").center();
    testing::click(&mut w, find, PointerButton::Primary, 1);
    assert!(w.state().ai.narrow.is_some(), "the panel opened");
    w.run_steps(2);
    let description = "invoices from Ada with a PDF";
    testing::text(&mut w, description);
    testing::key(&mut w, Key::Enter);
    w.run_steps(6);
    let asked = log.all();
    let models: Vec<&String> = asked.iter().filter(|l| l.starts_with("model ")).collect();
    assert_eq!(models.len(), 1, "one model request: {asked:?}");
    let job = models[0];
    assert!(job.contains(description), "the description went: {job}");
    for word in search.split_whitespace() {
        assert!(!job.to_lowercase().contains(word), "the search box's {word:?} went too: {job}");
    }
    assert!(job.contains("sources [] accounts []"), "no mail went with it: {job}");
    assert!(asked.iter().all(|l| l.starts_with("model ")), "nothing else was asked of the host: {asked:?}");
    assert_eq!(w.state().session.view.search, search, "the list's search stays until Use This Search");
}

// ---- Screenshots ---------------------------------------------------------------------------------

/// The Home tab with the AI group at 1024, 1280 and 1440 points, in light, dark, high contrast
/// and at 200 % (the window twice as large, so as wide in points), the simplified row and the
/// classic one, and with the keyboard on Ask Mail, as PNG files to look at:
/// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_the_ai_group -- --ignored`
#[test]
#[ignore = "writes screenshots, for looking at them"]
fn shots_of_the_ai_group() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    let looks = [("light", "light", false, 100u32), ("dark", "dark", false, 100), ("hc", "light", true, 100), ("200", "light", false, 200)];
    for (name, theme, contrast, size) in looks {
        for width in [1024.0, 1280.0, 1440.0] {
            for mode in [Mode::Simplified, Mode::Classic] {
                let (mut w, _) = window_with_host(width);
                let app = w.state_mut();
                app.execute("ui.theme", json!({"value": theme})).unwrap();
                app.execute("ui.highContrast", json!({"on": contrast})).unwrap();
                app.execute("ui.interfaceSize", json!({"value": size})).unwrap();
                app.execute("ui.ribbonMode", json!({"value": mode.code()})).unwrap();
                let scale = size as f32 / 100.0;
                w.set_size(vec2(width * scale, 800.0 * scale));
                w.run_steps(6);
                let file = format!("ai-group-{}-{name}-{width}.png", mode.code());
                w.render().unwrap().save(std::path::Path::new(&dir).join(file)).unwrap();
                if mode == Mode::Simplified && size == 100 && width == 1440.0 {
                    let rect = w.state().hit.rect("ribbon", hit_id("ui.answers")).unwrap();
                    if let Some(node) = w
                        .query_all_by_label("Ask Mail")
                        .find(|n| n.accesskit_node().role() == Role::Button && n.rect().center().distance(rect.center()) < 1.0)
                    {
                        node.focus();
                    }
                    w.run_steps(2);
                    w.render().unwrap().save(std::path::Path::new(&dir).join(format!("ai-group-focus-{name}.png"))).unwrap();
                }
            }
        }
    }
}
