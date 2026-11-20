mod api;
mod auth;
mod calendar;
mod home;
mod mensa;
mod mensa_ui;
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
    load_css(&[calendar::CSS, home::CSS]);
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
            *ui.state.borrow_mut() = Some(Rc::new(State { data: RefCell::new(data), access }));
            populate(&ui, Some("h:home"));
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

    let now = now_local();
    let mut folders: BTreeMap<(u8, String), Vec<(String, String, String)>> = BTreeMap::new();
    for c in &data.courses {
        let folder = if is_current(c) { (0, "This semester".into()) } else { (1, format!("Year {}", c.degree_year)) };
        let sub = match (c.passed(), c.next_exam(&now)) {
            (true, _) => grade(c),
            (false, Some((when, _))) => format!("Exam {}", fmt_local(&when, "%a %d %b")),
            (false, None) => c.status.clone(),
        };
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
    let home = adw::ActionRow::builder().title("Home").subtitle("Overview and statistics").name("h:home").build();
    home.add_prefix(&gtk::Image::from_icon_name("go-home-symbolic"));
    ui.list.append(&home);
    let cal = adw::ActionRow::builder().title("Calendar").subtitle("Lectures, exams and deadlines").name("k:calendar").build();
    cal.add_prefix(&gtk::Image::from_icon_name("x-office-calendar-symbolic"));
    ui.list.append(&cal);
    let mensa = adw::ActionRow::builder().title("Mensa").subtitle("Credit and menu").name("m:mensa").build();
    mensa.add_prefix(&gtk::Image::from_icon_name("emoji-food-symbolic"));
    ui.list.append(&mensa);
    let mut to_select = (select == Some("h:home")).then_some(home.upcast::<gtk::ListBoxRow>());
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
                to_select = Some(r.upcast());
            }
        }
    }
    drop((expanded, data));
    if let Some(r) = to_select {
        ui.list.select_row(Some(&r));
    }
}

/// Selects a sidebar entry, opening its folder first.
fn select_key(ui: &Ui, key: &str) {
    let mut folder = None;
    let mut i = 0;
    while let Some(r) = ui.list.row_at_index(i) {
        if r.widget_name().starts_with("f:") {
            folder = Some(r.clone());
        } else if r.widget_name() == key {
            if let Some(f) = folder.filter(|_| !r.is_visible()) {
                toggle_folder(ui, &f);
            }
            ui.list.select_row(Some(&r));
            return;
        }
        i += 1;
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
    let weak = Rc::downgrade(ui);
    let open: Rc<dyn Fn(&str)> = Rc::new(move |key| {
        if let Some(ui) = weak.upgrade() {
            select_key(&ui, key);
        }
    });
    let (title, page) = match kind {
        "k" => ("Calendar".to_owned(), toolbar(&calendar::page(&state.data.borrow(), &state.access, open), Some(adw::WindowTitle::new("Calendar", "").upcast_ref()), &[])),
        "h" => {
            let ole = state.access.moodle.clone();
            ("Home".to_owned(), toolbar(&home::page(&state.data.borrow(), ole, open).root, Some(adw::WindowTitle::new("Home", "").upcast_ref()), &[]))
        }
        "m" => ("Mensa".to_owned(), toolbar(&mensa_ui::page(), Some(adw::WindowTitle::new("Mensa", "").upcast_ref()), &[])),
        "c" => {
            let data = state.data.borrow();
            let Some(c) = data.courses.iter().find(|c| c.id.to_string() == id).cloned() else { return };
            drop(data);
            (c.name.clone(), course_page(ui, &state, &c))
        }
        "o" => {
            let name = state.data.borrow().ole.iter().find(|o| o.0.to_string() == id).map(|o| o.1.clone()).unwrap_or_default();
            let id = id.parse().unwrap_or_default();
            let menu = link_menu(ui, &state, Link::Ole(id, name.clone()), None);
            (name.clone(), tabs(vec![("OLE".into(), "accessories-dictionary-symbolic", ole_tab(&state, id, &name, &name, menu))], &[]))
        }
        "t" => {
            let Some(t) = state.data.borrow().teams.iter().find(|t| t.0 == id).cloned() else { return };
            let menu = link_menu(ui, &state, Link::Team(t.clone()), None);
            (t.1.clone(), tabs(vec![("Teams".into(), "system-users-symbolic", team_tab(&state, &t, &t.1, menu))], &[]))
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
    toolbar(&with_preview(&stack), Some(switcher.upcast_ref()), extra)
}

fn ole_tab(state: &State, id: i64, name: &str, folder: &str, menu: gtk::Widget) -> gtk::Widget {
    match &state.access.moodle {
        Ok(token) => ole_ui::tab(Rc::new(ole_ui::Ole {
            token: token.clone(),
            private: auth::stored("moodle_private"),
            user: state.data.borrow().ole_user,
            course: id,
            name: name.into(),
            folder: folder.into(),
        }), &[menu])
        .upcast(),
        Err(e) => status("OLE unavailable", e, false).upcast(),
    }
}

fn team_tab(state: &State, t: &(String, String, String), folder: &str, menu: gtk::Widget) -> gtk::Widget {
    match &state.access.graph {
        Ok(token) => teams_ui::tab(Rc::new(teams_ui::Team { token: token.clone(), id: t.0.clone(), name: t.1.clone(), url: t.2.clone(), folder: folder.into() }), &[menu]).upcast(),
        Err(e) => status("Teams unavailable", e, false).upcast(),
    }
}

fn course_page(ui: &Rc<Ui>, state: &Rc<State>, c: &Course) -> adw::ToolbarView {
    let mut pages = vec![("mySNS".to_string(), "x-office-document-symbolic", overview(ui, state, c).root.upcast())];
    for (id, name) in &c.moodle {
        let title = if c.moodle.len() > 1 { format!("OLE: {name}") } else { "OLE".into() };
        let menu = link_menu(ui, state, Link::Ole(*id, name.clone()), Some(c.id));
        pages.push((title, "accessories-dictionary-symbolic", ole_tab(state, *id, name, &c.name, menu)));
    }
    for t in &c.teams {
        let title = if c.teams.len() > 1 { format!("Teams: {}", t.1) } else { "Teams".into() };
        let menu = link_menu(ui, state, Link::Team(t.clone()), Some(c.id));
        pages.push((title, "system-users-symbolic", team_tab(state, t, &c.name, menu)));
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
    save_link(ui, state, course_id, link, add, true)
}

/// `persist`: write an override even if the course had none (a new enrolment is found again automatically).
fn save_link(ui: &Rc<Ui>, state: &State, course_id: i64, link: Link, add: bool, persist: bool) {
    let mut data = state.data.borrow_mut();
    let Some(c) = data.courses.iter_mut().find(|c| c.id == course_id) else { return };
    match (link, add) {
        (Link::Ole(id, name), true) => {
            if !c.moodle.iter().any(|m| m.0 == id) {
                c.moodle.push((id, name))
            }
        }
        (Link::Ole(id, _), false) => c.moodle.retain(|m| m.0 != id),
        (Link::Team(t), true) => {
            if !c.teams.iter().any(|x| x.0 == t.0) {
                c.teams.push(t)
            }
        }
        (Link::Team(t), false) => c.teams.retain(|x| x.0 != t.0),
    }
    let mut cfg = auth::config();
    if persist || cfg.overrides.contains_key(&c.code) {
        cfg.overrides.insert(c.code.clone(), auth::Override { moodle: c.moodle.iter().map(|m| m.0).collect(), teams: c.teams.iter().map(|t| t.0.clone()).collect() });
        if let Err(e) = auth::save_config(&cfg) {
            eprintln!("saving config: {e}");
        }
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

/// "Assign" menu for an OLE course or team; `current` is the mySNS course it is shown under.
fn link_menu(ui: &Rc<Ui>, state: &Rc<State>, link: Link, current: Option<i64>) -> gtk::Widget {
    let menu = gtk::MenuButton::builder().label("Assign").css_classes(["flat"]).valign(gtk::Align::Center).tooltip_text("Assign to mySNS courses").build();
    let popover = gtk::Popover::new();
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    #[derive(Clone, Copy)]
    enum Mode {
        Add,
        Move,
        Remove,
    }
    let mut entries = vec![("Assign to another course…", Mode::Add)];
    if current.is_some() {
        entries.push(("Move to another course…", Mode::Move));
        entries.push(("Unlink from this course", Mode::Remove));
    }
    for (label, mode) in entries {
        let b = gtk::Button::builder().label(label).css_classes(["flat"]).build();
        b.child().and_downcast::<gtk::Label>().inspect(|l| l.set_xalign(0.0));
        let (ui, state, link, popover2, menu2) = (ui.clone(), state.clone(), link.clone(), popover.clone(), menu.clone());
        b.connect_clicked(move |_| {
            popover2.popdown();
            if let (Mode::Remove, Some(cur)) = (mode, current) {
                return change_link(&ui, &state, cur, link.clone(), false);
            }
            let options = state
                .data
                .borrow()
                .courses
                .iter()
                .filter(|c| Some(c.id) != current)
                .map(|c| {
                    let (ui, state, link, target) = (ui.clone(), state.clone(), link.clone(), c.id);
                    let act = move || {
                        if let (Mode::Move, Some(cur)) = (mode, current) {
                            change_link(&ui, &state, cur, link.clone(), false);
                        }
                        change_link(&ui, &state, target, link.clone(), true);
                    };
                    (c.name.clone(), format!("{} · year {}", c.status, c.degree_year), Box::new(act) as Box<dyn Fn()>)
                })
                .collect();
            choose(&menu2, if matches!(mode, Mode::Move) { "Move to mySNS course" } else { "Assign to mySNS course" }, options);
        });
        vbox.append(&b);
    }
    popover.set_child(Some(&vbox));
    menu.set_popover(Some(&popover));
    menu.upcast()
}

/// After enrolling/joining: it is no longer a suggestion anywhere, and becomes a linked tab here.
fn enrolled(ui: &Rc<Ui>, state: &Rc<State>, course_id: i64, link: Link) {
    {
        let mut d = state.data.borrow_mut();
        match &link {
            Link::Ole(id, name) => {
                d.ole.push((*id, name.clone()));
                d.courses.iter_mut().for_each(|c| c.suggest_moodle.retain(|m| m.0 != *id));
            }
            Link::Team(t) => {
                d.teams.push(t.clone());
                d.courses.iter_mut().for_each(|c| c.suggest_teams.retain(|x| x.0 != t.0));
            }
        }
    }
    save_link(ui, state, course_id, link, true, false);
}

fn suggestions(ui: &Rc<Ui>, state: &Rc<State>, c: &Course) -> Option<adw::PreferencesGroup> {
    if c.suggest_moodle.is_empty() && c.suggest_teams.is_empty() {
        return None;
    }
    let g = group("Suggested by mySNS");
    g.set_description(Some("The mySNS timetable links these to this course, but you are not enrolled yet."));
    for (id, name) in &c.suggest_moodle {
        let r = row(name, "OLE course · not enrolled");
        r.add_suffix(&link_button("Open", format!("{}/course/view.php?id={id}", auth::OLE)));
        let (ui, state, cid, id, name) = (ui.clone(), state.clone(), c.id, *id, name.clone());
        r.add_suffix(&button("Enrol", move |b| {
            let Ok(token) = state.access.moodle.clone() else { return alert(b, "OLE unavailable", "Not signed in to OLE.") };
            let (ui, state, name, b) = (ui.clone(), state.clone(), name.clone(), b.clone());
            b.set_sensitive(false);
            glib::spawn_future_local(async move {
                let mut key: Option<String> = None;
                loop {
                    let (t, k) = (token.clone(), key.clone());
                    match gio::spawn_blocking(move || moodle::self_enrol(&t, id, k.as_deref())).await.unwrap_or_else(|_| Err("panicked".into())) {
                        Ok(()) => return enrolled(&ui, &state, cid, Link::Ole(id, name)),
                        // Usually means an enrolment key is required (or the one given was wrong).
                        Err(e) if !e.contains("no self-enrolment") => match ask_line(&b, "Enrolment key", &e, "Key from the lecturer").await {
                            Some(k) => key = Some(k),
                            None => break,
                        },
                        Err(e) => {
                            alert(&b, "Could not enrol", &e);
                            break;
                        }
                    }
                }
                b.set_sensitive(true);
            });
        }));
        g.add(&r);
    }
    for (gid, name, url, code) in &c.suggest_teams {
        let r = row(name, &if code.is_empty() { "Team · not joined".into() } else { format!("Team · not joined · join code {code}") });
        let (ui, state, cid, team, code) = (ui.clone(), state.clone(), c.id, (gid.clone(), name.clone(), url.clone()), code.clone());
        r.add_suffix(&button("Join", move |b| {
            let Ok(token) = state.access.graph.clone() else { return alert(b, "Teams unavailable", "Not signed in to Microsoft.") };
            let (ui, state, team, code, b) = (ui.clone(), state.clone(), team.clone(), code.clone(), b.clone());
            b.set_sensitive(false);
            let gid = team.0.clone();
            spawn(
                move || {
                    let me = api::get(&format!("{}/me?$select=id", api::GRAPH), Some(&token))?;
                    let body = serde_json::json!({ "@odata.id": format!("{}/directoryObjects/{}", api::GRAPH, api::s(&me["id"])) });
                    api::post_json(&format!("{}/groups/{gid}/members/$ref", api::GRAPH), Some(&token), &body)
                },
                move |r| match r {
                    Ok(_) => enrolled(&ui, &state, cid, Link::Team(team)),
                    Err(e) => {
                        b.set_sensitive(true);
                        // Private class teams only accept the join code, which Graph cannot redeem.
                        let dialog = adw::AlertDialog::new(
                            Some("Join in Teams"),
                            Some(&format!("Joining automatically was refused ({}).\n\nJoin code: {}", e.chars().take(160).collect::<String>(), if code.is_empty() { "none in mySNS" } else { &code })),
                        );
                        dialog.add_responses(&[("close", "Close"), ("open", "Copy code and open Teams")]);
                        dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
                        let (b2, url) = (b.clone(), team.2.clone());
                        dialog.connect_response(Some("open"), move |_, _| {
                            b2.clipboard().set_text(&code);
                            open_uri(&b2, if url.is_empty() { "https://teams.microsoft.com/" } else { &url });
                        });
                        dialog.present(Some(&b));
                    }
                },
            );
        }));
        g.add(&r);
    }
    Some(g)
}

fn overview(ui: &Rc<Ui>, state: &Rc<State>, c: &Course) -> WidePage {
    let page = WidePage::new();
    let data = state.data.borrow();

    let now = now_local();
    let g = group("Result");
    if c.passed() {
        g.add(&row("Grade", &grade(c)));
        if let Some(d) = &c.exam_date {
            g.add(&row("Exam date", d));
        }
    } else {
        match c.next_exam(&now) {
            Some((when, desc)) => g.add(&row(&format!("Next exam: {}", fmt_local(&when, "%A %d %B %Y, %H:%M")), &desc)),
            None => g.add(&row("Next exam", "No exam appointment in mySNS yet")),
        }
        g.add(&row("Status", &c.status));
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

    let g = group("Exams");
    let exam_slots: Vec<_> = c.lectures.iter().filter(|l| l.kind == "EXAM").collect();
    if c.exams.is_empty() && exam_slots.is_empty() {
        g.add(&row("No exam appointments", "From the mySNS calendar and exam enrolment"));
    }
    for l in exam_slots {
        g.add(&row(&format!("{} · {}", fmt_local(&l.start, "%a %d %b %Y, %H:%M"), l.room), &format!("{} · {}", l.activity, l.staff)));
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

    let lectures: Vec<_> = c.lectures.iter().filter(|l| l.kind != "EXAM").collect();
    let upcoming: Vec<_> = lectures.iter().filter(|l| l.end >= now).collect();
    let g = group("Lectures");
    g.set_description(Some(&format!("{} upcoming, {} past this academic year", upcoming.len(), lectures.len() - upcoming.len())));
    for l in upcoming.iter().take(30) {
        let date = fmt_local(&l.start, "%a %d %b, %H:%M");
        let end = l.end.get(11..16).unwrap_or("");
        g.add(&row(&format!("{date}–{end} · {}", l.room), &format!("{} · {}", l.activity, l.staff)));
    }
    page.add(&g);

    if let Some(g) = suggestions(ui, state, c) {
        page.add(&g);
    }
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
