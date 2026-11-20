mod api;
mod auth;
mod moodle;
mod ole_ui;
mod teams_ui;
mod ui;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    rc::Rc,
};

use adw::prelude::*;
use api::{Access, Course, Data};
use auth::Res;
use gtk::{gio, glib};
use ui::*;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id("it.unibz.Terminal").build();
    app.connect_activate(build);
    app.run()
}

struct State {
    data: RefCell<Data>,
    access: Access,
}

struct Ui {
    win: adw::ApplicationWindow,
    split: adw::NavigationSplitView,
    list: gtk::ListBox,
    content: adw::NavigationPage,
    state: RefCell<Option<Rc<State>>>,
    expanded: RefCell<HashSet<String>>,
}

fn toolbar(child: &impl IsA<gtk::Widget>, title: Option<&gtk::Widget>, extra: &[gtk::Widget]) -> adw::ToolbarView {
    let header = adw::HeaderBar::new();
    if let Some(t) = title {
        header.set_title_widget(Some(t));
    }
    for w in extra {
        header.pack_start(w);
    }
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(child));
    tv
}

fn status(title: &str, desc: &str, spinner: bool) -> adw::StatusPage {
    let p = adw::StatusPage::builder().title(title).description(esc(desc)).build();
    if spinner {
        p.set_paintable(Some(&adw::SpinnerPaintable::new(Some(&p))));
    }
    p
}

fn build(app: &adw::Application) {
    let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Reload"));
    let sidebar = adw::NavigationPage::builder()
        .title("Courses")
        .child(&toolbar(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build(), None, &[refresh.clone().upcast()]))
        .build();
    let content = adw::NavigationPage::builder().title("unibz").build();
    let split = adw::NavigationSplitView::builder().sidebar(&sidebar).content(&content).build();
    let win = adw::ApplicationWindow::builder().application(app).title("unibz terminal").default_width(1200).default_height(800).content(&split).build();
    let ui = Rc::new(Ui { win, split, list, content, state: RefCell::default(), expanded: RefCell::new(HashSet::from(["This semester".into()])) });

    ui.list.connect_row_activated(glib::clone!(#[weak] ui, move |_, row| {
        if row.widget_name().starts_with("f:") {
            toggle_folder(&ui, row);
        }
    }));
    ui.list.connect_row_selected(glib::clone!(#[weak] ui, move |_, row| {
        if let Some(r) = row {
            show(&ui, &r.widget_name());
        }
    }));
    refresh.connect_clicked(glib::clone!(#[strong] ui, move |_| {
        glib::spawn_future_local(load(ui.clone()));
    }));
    ui.win.present();
    glib::spawn_future_local(load(ui));
}

async fn load(ui: Rc<Ui>) {
    ui.list.remove_all();
    ui.content.set_child(Some(&toolbar(&status("Loading", "Signing in and fetching mySNS, OLE and Teams…", true), None, &[])));
    match fetch().await {
        Ok((data, access)) => {
            let msg = format!("{} courses in your study plan.", data.courses.len());
            let warnings = if data.warnings.is_empty() { String::new() } else { format!("\n\nProblems:\n{}", data.warnings.join("\n")) };
            *ui.state.borrow_mut() = Some(Rc::new(State { data: RefCell::new(data), access }));
            populate(&ui, None);
            ui.content.set_child(Some(&toolbar(&status("Pick a course", &format!("{msg}{warnings}"), false), None, &[])));
        }
        Err(e) => ui.content.set_child(Some(&toolbar(&status("Could not load mySNS", &e, false), None, &[]))),
    }
}

async fn fetch() -> Res<(Data, Access)> {
    let cfg = auth::config();
    let sns = auth::sns();
    let cockpit = auth::access_token(&sns, auth::COCKPIT).await?;
    let exams = auth::access_token(&sns, auth::EXAMS).await;
    let moodle = auth::moodle_token(&cfg).await;
    let graph = auth::access_token(&auth::graph(&cfg), auth::GRAPH).await;
    let access = Access { cockpit, exams, graph, moodle };
    let a = access.clone();
    let data = gio::spawn_blocking(move || api::load(&a, &cfg)).await.map_err(|_| "loader panicked")??;
    Ok((data, access))
}

fn grade(c: &Course) -> String {
    match c.mark {
        Some(m) if c.cum_laude => format!("{m} cum laude"),
        Some(m) => m.to_string(),
        None => c.status.clone(),
    }
}

fn is_current(c: &Course) -> bool {
    let cutoff = glib::DateTime::now_local().and_then(|d| d.add_days(-14)).ok().and_then(|d| d.format("%Y-%m-%dT%H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
    !c.exams.is_empty() || c.lectures.iter().any(|l| l.end >= cutoff)
}

/// Rebuilds the sidebar as folders; `select` is a row key to (re)select.
fn populate(ui: &Rc<Ui>, select: Option<&str>) {
    let Some(state) = ui.state.borrow().clone() else { return };
    let data = state.data.borrow();
    ui.list.remove_all();

    let mut folders: BTreeMap<(u8, String), Vec<(String, String, String)>> = BTreeMap::new();
    for c in &data.courses {
        let folder = if is_current(c) { (0, "This semester".into()) } else { (1, format!("Year {}", c.degree_year)) };
        let sub = if c.mark.is_some() { grade(c) } else { c.status.clone() };
        folders.entry(folder).or_default().push((format!("c:{}", c.id), c.name.clone(), sub));
    }
    let linked_ole: HashSet<i64> = data.courses.iter().flat_map(|c| c.moodle.iter().map(|m| m.0)).collect();
    let linked_teams: HashSet<&str> = data.courses.iter().flat_map(|c| c.teams.iter().map(|t| t.0.as_str())).collect();
    for (id, name) in data.ole.iter().filter(|(id, _)| !linked_ole.contains(id)) {
        folders.entry((2, "Other OLE courses".into())).or_default().push((format!("o:{id}"), name.clone(), "OLE".into()));
    }
    for (id, name, _) in data.teams.iter().filter(|t| !linked_teams.contains(t.0.as_str())) {
        folders.entry((3, "Other teams".into())).or_default().push((format!("t:{id}"), name.clone(), "Teams".into()));
    }

    let expanded = ui.expanded.borrow();
    let mut to_select = None;
    for ((_, folder), mut entries) in folders {
        entries.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        let open = expanded.contains(&folder) || select.is_some_and(|k| entries.iter().any(|e| e.0 == k));
        let hbox = gtk::Box::builder().spacing(8).margin_top(6).margin_bottom(6).margin_start(6).build();
        hbox.append(&gtk::Image::from_icon_name(if open { "pan-down-symbolic" } else { "pan-end-symbolic" }));
        hbox.append(&gtk::Label::builder().label(&folder).css_classes(["heading"]).hexpand(true).xalign(0.0).build());
        hbox.append(&gtk::Label::builder().label(entries.len().to_string()).css_classes(["dim-label"]).margin_end(6).build());
        let frow = gtk::ListBoxRow::builder().child(&hbox).selectable(false).name(format!("f:{folder}")).build();
        ui.list.append(&frow);
        for (key, title, sub) in entries {
            let r = adw::ActionRow::builder().title(esc(&title)).subtitle(esc(&sub)).name(&key).margin_start(18).visible(open).build();
            ui.list.append(&r);
            if select == Some(key.as_str()) {
                to_select = Some(r);
            }
        }
    }
    drop((expanded, data));
    if let Some(r) = to_select {
        ui.list.select_row(Some(&r));
    }
}

fn toggle_folder(ui: &Ui, folder: &gtk::ListBoxRow) {
    let name = folder.widget_name().trim_start_matches("f:").to_owned();
    let open = !ui.expanded.borrow().contains(&name);
    if open {
        ui.expanded.borrow_mut().insert(name);
    } else {
        ui.expanded.borrow_mut().remove(&name);
    }
    if let Some(icon) = folder.child().and_then(|b| b.first_child()).and_downcast::<gtk::Image>() {
        icon.set_icon_name(Some(if open { "pan-down-symbolic" } else { "pan-end-symbolic" }));
    }
    let mut i = folder.index() + 1;
    while let Some(r) = ui.list.row_at_index(i) {
        if r.widget_name().starts_with("f:") {
            break;
        }
        r.set_visible(open);
        i += 1;
    }
}

fn show(ui: &Rc<Ui>, key: &str) {
    let Some(state) = ui.state.borrow().clone() else { return };
    let (kind, id) = key.split_once(':').unwrap_or_default();
    let (title, page) = match kind {
        "c" => {
            let data = state.data.borrow();
            let Some(c) = data.courses.iter().find(|c| c.id.to_string() == id).cloned() else { return };
            drop(data);
            (c.name.clone(), course_page(ui, &state, &c))
        }
        "o" => {
            let name = state.data.borrow().ole.iter().find(|o| o.0.to_string() == id).map(|o| o.1.clone()).unwrap_or_default();
            let assign = assign_to_course_button(ui, &state, Link::Ole(id.parse().unwrap_or_default(), name.clone()));
            (name.clone(), tabs(vec![("OLE".into(), "accessories-dictionary-symbolic", ole_tab(&state, id.parse().unwrap_or_default(), &name, &name))], &[assign]))
        }
        "t" => {
            let Some(t) = state.data.borrow().teams.iter().find(|t| t.0 == id).cloned() else { return };
            let assign = assign_to_course_button(ui, &state, Link::Team(t.clone()));
            (t.1.clone(), tabs(vec![("Teams".into(), "system-users-symbolic", team_tab(&state, &t, &t.1))], &[assign]))
        }
        _ => return,
    };
    ui.content.set_title(&title);
    ui.content.set_child(Some(&page));
    ui.split.set_show_content(true);
}

/// A header bar with a view switcher over the given tabs.
fn tabs(pages: Vec<(String, &str, gtk::Widget)>, extra: &[gtk::Widget]) -> adw::ToolbarView {
    let stack = adw::ViewStack::new();
    for (i, (title, icon, w)) in pages.iter().enumerate() {
        stack.add_titled_with_icon(w, Some(&format!("tab{i}")), title, icon);
    }
    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    toolbar(&stack, Some(switcher.upcast_ref()), extra)
}

fn ole_tab(state: &State, id: i64, name: &str, folder: &str) -> gtk::Widget {
    match &state.access.moodle {
        Ok(token) => ole_ui::tab(Rc::new(ole_ui::Ole {
            token: token.clone(),
            private: auth::stored("moodle_private"),
            user: state.data.borrow().ole_user,
            course: id,
            name: name.into(),
            folder: folder.into(),
        }))
        .upcast(),
        Err(e) => status("OLE unavailable", e, false).upcast(),
    }
}

fn team_tab(state: &State, t: &(String, String, String), folder: &str) -> gtk::Widget {
    match &state.access.graph {
        Ok(token) => teams_ui::tab(Rc::new(teams_ui::Team { token: token.clone(), id: t.0.clone(), name: t.1.clone(), url: t.2.clone(), folder: folder.into() })).upcast(),
        Err(e) => status("Teams unavailable", e, false).upcast(),
    }
}

fn course_page(ui: &Rc<Ui>, state: &Rc<State>, c: &Course) -> adw::ToolbarView {
    let mut pages = vec![("mySNS".to_string(), "x-office-document-symbolic", overview(ui, state, c).upcast())];
    for (id, name) in &c.moodle {
        let title = if c.moodle.len() > 1 { format!("OLE: {name}") } else { "OLE".into() };
        pages.push((title, "accessories-dictionary-symbolic", ole_tab(state, *id, name, &c.name)));
    }
    for t in &c.teams {
        let title = if c.teams.len() > 1 { format!("Teams: {}", t.1) } else { "Teams".into() };
        pages.push((title, "system-users-symbolic", team_tab(state, t, &c.name)));
    }
    tabs(pages, &[])
}

#[derive(Clone)]
enum Link {
    Ole(i64, String),
    Team((String, String, String)),
}

/// Adds/removes a link on a course, saves it as an override, and refreshes the sidebar.
fn change_link(ui: &Rc<Ui>, state: &State, course_id: i64, link: Link, add: bool) {
    let mut data = state.data.borrow_mut();
    let Some(c) = data.courses.iter_mut().find(|c| c.id == course_id) else { return };
    match (link, add) {
        (Link::Ole(id, name), true) => c.moodle.push((id, name)),
        (Link::Ole(id, _), false) => c.moodle.retain(|m| m.0 != id),
        (Link::Team(t), true) => c.teams.push(t),
        (Link::Team(t), false) => c.teams.retain(|x| x.0 != t.0),
    }
    let mut cfg = auth::config();
    cfg.overrides.insert(c.code.clone(), auth::Override { moodle: c.moodle.iter().map(|m| m.0).collect(), teams: c.teams.iter().map(|t| t.0.clone()).collect() });
    if let Err(e) = auth::save_config(&cfg) {
        eprintln!("saving config: {e}");
    }
    drop(data);
    populate(ui, Some(&format!("c:{course_id}")));
}

/// A searchable picker dialog.
fn choose(parent: &impl IsA<gtk::Widget>, title: &str, options: Vec<(String, String, Box<dyn Fn()>)>) {
    let dialog = adw::Dialog::builder().title(title).content_width(460).content_height(560).build();
    let search = gtk::SearchEntry::builder().margin_start(12).margin_end(12).margin_top(6).margin_bottom(6).build();
    let list = gtk::ListBox::builder().css_classes(["boxed-list"]).margin_start(12).margin_end(12).margin_bottom(12).valign(gtk::Align::Start).build();
    let mut acts = Vec::new();
    for (label, sub, act) in options {
        list.append(&adw::ActionRow::builder().title(esc(&label)).subtitle(esc(&sub)).activatable(true).build());
        acts.push(act);
    }
    let acts = Rc::new(acts);
    list.connect_row_activated(glib::clone!(#[weak] dialog, move |_, r| {
        if let Some(a) = acts.get(r.index() as usize) {
            a();
        }
        dialog.close();
    }));
    let s2 = search.clone();
    list.set_filter_func(move |r| {
        let q = s2.text().to_lowercase();
        q.is_empty() || r.downcast_ref::<adw::ActionRow>().is_some_and(|a| format!("{} {}", a.title(), a.subtitle().unwrap_or_default()).to_lowercase().contains(&q))
    });
    search.connect_search_changed(glib::clone!(#[weak] list, move |_| list.invalidate_filter()));
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&search);
    vbox.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build());
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&vbox));
    dialog.set_child(Some(&tv));
    dialog.present(Some(parent));
}

fn assign_to_course_button(ui: &Rc<Ui>, state: &Rc<State>, link: Link) -> gtk::Widget {
    let (ui, state) = (ui.clone(), state.clone());
    button("Assign to course…", move |b| {
        let options = state
            .data
            .borrow()
            .courses
            .iter()
            .map(|c| {
                let (ui, state, link, id) = (ui.clone(), state.clone(), link.clone(), c.id);
                (c.name.clone(), format!("{} · year {}", c.status, c.degree_year), Box::new(move || change_link(&ui, &state, id, link.clone(), true)) as Box<dyn Fn()>)
            })
            .collect();
        choose(b, "Assign to mySNS course", options);
    })
    .upcast()
}

fn overview(ui: &Rc<Ui>, state: &Rc<State>, c: &Course) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let data = state.data.borrow();

    let g = group("Grade");
    g.add(&row("Status", &grade(c)));
    if let Some(d) = &c.exam_date {
        g.add(&row("Exam date", d));
    }
    g.add(&row("Credits", &format!("{} ECTS · year {} · code {}", c.credits, c.degree_year, c.code)));
    if let Some(u) = &c.syllabus {
        g.set_header_suffix(Some(&link_button("Syllabus", u.clone())));
    }
    if c.has_history {
        let (a, sid, q, id) = (state.access.clone(), data.student_id, data.query.clone(), c.id);
        fill(&g, move || api::history(&a, sid, &q, id), |g, attempts| {
            for (date, result) in attempts {
                g.add(&row(&format!("Attempt {date}"), &result));
            }
        });
    }
    page.add(&g);

    let g = group("Exam sessions");
    if c.exams.is_empty() {
        g.add(&row("No exam sessions open", "From mySNS exam enrolment"));
    }
    for e in &c.exams {
        let f = |k: &str| api::s(&e[k]);
        let title = [f("examDescription"), f("examTypeDescription")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" — ");
        let when = format!("{} {}", f("examDate").chars().take(10).collect::<String>(), f("examTime"));
        let sub = [when.trim().to_owned(), f("location"), f("examProfessorName")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        let r = row(&title, &sub);
        if e["enrolmentDone"].as_bool() == Some(true) {
            r.add_suffix(&gtk::Label::builder().label("Enrolled").css_classes(["success"]).build());
        }
        g.add(&r);
    }
    page.add(&g);

    let now = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%dT%H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
    let upcoming: Vec<_> = c.lectures.iter().filter(|l| l.end >= now).collect();
    let g = group("Lectures");
    g.set_description(Some(&format!("{} upcoming, {} past this academic year", upcoming.len(), c.lectures.len() - upcoming.len())));
    for l in upcoming.iter().take(30) {
        let date = glib::DateTime::from_iso8601(&l.start, Some(&glib::TimeZone::local()))
            .ok()
            .and_then(|d| d.format("%a %d %b, %H:%M").ok())
            .map_or(l.start.clone(), |s| s.to_string());
        let end = l.end.get(11..16).unwrap_or("");
        g.add(&row(&format!("{date}–{end} · {}", l.room), &format!("{} · {}", l.activity, l.staff)));
    }
    page.add(&g);

    let g = group("Linked OLE courses and teams");
    g.set_description(Some("Changes are saved to ~/.config/unibz-terminal/config.json"));
    let links: Vec<(String, &str, Link)> = c
        .moodle
        .iter()
        .map(|(id, n)| (n.clone(), "OLE", Link::Ole(*id, n.clone())))
        .chain(c.teams.iter().map(|t| (t.1.clone(), "Teams", Link::Team(t.clone()))))
        .collect();
    if links.is_empty() {
        g.add(&row("Nothing linked", "Use “Assign…” to link an OLE course or team"));
    }
    for (name, kind, link) in links {
        let r = row(&name, kind);
        let rm = gtk::Button::builder().icon_name("list-remove-symbolic").tooltip_text("Unlink").css_classes(["flat"]).valign(gtk::Align::Center).build();
        let (ui2, state2, id) = (ui.clone(), state.clone(), c.id);
        rm.connect_clicked(move |_| change_link(&ui2, &state2, id, link.clone(), false));
        r.add_suffix(&rm);
        g.add(&r);
    }
    let (ui2, state2, cid) = (ui.clone(), state.clone(), c.id);
    let (have_ole, have_teams): (HashSet<i64>, HashSet<String>) = (c.moodle.iter().map(|m| m.0).collect(), c.teams.iter().map(|t| t.0.clone()).collect());
    g.set_header_suffix(Some(&button("Assign…", move |b| {
        let data = state2.data.borrow();
        let mut options: Vec<(String, String, Box<dyn Fn()>)> = Vec::new();
        for (id, name) in data.ole.iter().filter(|o| !have_ole.contains(&o.0)) {
            let (ui, st, link) = (ui2.clone(), state2.clone(), Link::Ole(*id, name.clone()));
            options.push((name.clone(), "OLE course".into(), Box::new(move || change_link(&ui, &st, cid, link.clone(), true))));
        }
        for t in data.teams.iter().filter(|t| !have_teams.contains(&t.0)) {
            let (ui, st, link) = (ui2.clone(), state2.clone(), Link::Team(t.clone()));
            options.push((t.1.clone(), "Team".into(), Box::new(move || change_link(&ui, &st, cid, link.clone(), true))));
        }
        drop(data);
        choose(b, "Link OLE course or team", options);
    })));
    page.add(&g);
    page
}
