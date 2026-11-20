mod api;
mod auth;

use std::rc::Rc;

use adw::prelude::*;
use api::{Access, Course, Data, Item};
use auth::Res;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id("it.unibz.Terminal").build();
    app.connect_activate(build);
    app.run()
}

struct Ui {
    win: adw::ApplicationWindow,
    split: adw::NavigationSplitView,
    list: gtk::ListBox,
    content: adw::NavigationPage,
}

fn toolbar(child: &impl IsA<gtk::Widget>, extra: Option<&gtk::Button>) -> adw::ToolbarView {
    let header = adw::HeaderBar::new();
    if let Some(b) = extra {
        header.pack_start(b);
    }
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(child));
    tv
}

fn status(title: &str, desc: &str, spinner: bool) -> adw::StatusPage {
    let p = adw::StatusPage::builder().title(title).description(glib::markup_escape_text(desc)).build();
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
        .child(&toolbar(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build(), Some(&refresh)))
        .build();
    let content = adw::NavigationPage::builder().title("unibz").build();
    let split = adw::NavigationSplitView::builder().sidebar(&sidebar).content(&content).build();
    let win = adw::ApplicationWindow::builder().application(app).title("unibz terminal").default_width(1150).default_height(780).content(&split).build();
    let ui = Rc::new(Ui { win, split, list, content });
    refresh.connect_clicked(glib::clone!(#[strong] ui, move |_| { glib::spawn_future_local(load(ui.clone())); }));
    ui.win.present();
    glib::spawn_future_local(load(ui));
}

async fn load(ui: Rc<Ui>) {
    ui.list.remove_all();
    ui.content.set_child(Some(&toolbar(&status("Loading", "Signing in and fetching mySNS, OLE and Teams…", true), None)));
    match fetch().await {
        Ok((data, access)) => populate(&ui, Rc::new(data), Rc::new(access)),
        Err(e) => ui.content.set_child(Some(&toolbar(&status("Could not load mySNS", &e, false), None))),
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

fn populate(ui: &Rc<Ui>, data: Rc<Data>, access: Rc<Access>) {
    for c in &data.courses {
        let sub = match (c.mark, c.lectures.is_empty()) {
            (Some(_), _) => grade(c),
            (None, false) => format!("{} · {} lectures", c.status, c.lectures.len()),
            (None, true) => c.status.clone(),
        };
        let row = adw::ActionRow::builder().title(glib::markup_escape_text(&c.name)).subtitle(sub).build();
        ui.list.append(&row);
    }
    let warnings = if data.warnings.is_empty() { String::new() } else { format!("\n\nProblems:\n{}", data.warnings.join("\n")) };
    ui.content.set_child(Some(&toolbar(&status("Pick a course", &format!("{} courses in your study plan.{warnings}", data.courses.len()), false), None)));
    ui.list.connect_row_selected(glib::clone!(#[weak] ui, move |_, row| {
        if let Some(c) = row.and_then(|r| data.courses.get(r.index() as usize)) {
            show_course(&ui, c, &data, &access);
        }
    }));
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(glib::markup_escape_text(title)).build()
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder().title(glib::markup_escape_text(title)).subtitle(glib::markup_escape_text(subtitle)).build()
}

fn link_button(label: &str, url: String) -> gtk::Button {
    let b = gtk::Button::builder().label(label).css_classes(["flat"]).valign(gtk::Align::Center).build();
    b.connect_clicked(move |b| open_uri(b, &url));
    b
}

fn open_uri(w: &impl IsA<gtk::Widget>, url: &str) {
    gtk::UriLauncher::new(url).launch(w.root().and_downcast_ref::<gtk::Window>(), None::<&gio::Cancellable>, |r| {
        if let Err(e) = r {
            eprintln!("open: {e}");
        }
    });
}

/// Fills `g` with the result of a blocking fetch, showing progress/errors in place.
fn fill_async<T: Send + 'static>(g: &adw::PreferencesGroup, job: impl FnOnce() -> Res<T> + Send + 'static, render: impl FnOnce(&adw::PreferencesGroup, T) + 'static) {
    let placeholder = row("Loading…", "");
    g.add(&placeholder);
    let g = g.clone();
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(job).await.unwrap_or_else(|_| Err("panicked".into()));
        g.remove(&placeholder);
        match r {
            Ok(v) => render(&g, v),
            Err(e) => g.add(&row("Could not load", &e)),
        }
    });
}

fn item_row(it: &Item, course: &str) -> adw::ActionRow {
    let r = row(&it.title, &it.subtitle);
    r.set_activatable(true);
    r.add_suffix(&gtk::Image::from_icon_name(if it.download.is_some() { "folder-download-symbolic" } else { "adw-external-link-symbolic" }));
    let (it, course) = (it.clone(), course.to_owned());
    r.connect_activated(move |r| match it.download.clone() {
        Some((url, name)) => {
            let (r, sub, course) = (r.clone(), it.subtitle.clone(), course.clone());
            r.set_subtitle("Downloading…");
            glib::spawn_future_local(async move {
                match gio::spawn_blocking(move || api::download(&url, &course, &name)).await.unwrap_or_else(|_| Err("panicked".into())) {
                    Ok(path) => {
                        r.set_subtitle(&glib::markup_escape_text(&sub));
                        gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(r.root().and_downcast_ref::<gtk::Window>(), None::<&gio::Cancellable>, |_| {});
                    }
                    Err(e) => r.set_subtitle(&glib::markup_escape_text(&format!("Download failed: {e}"))),
                }
            });
        }
        None => {
            if let Some(u) = &it.web {
                open_uri(r, u);
            }
        }
    });
    r
}

fn show_course(ui: &Ui, c: &Course, data: &Rc<Data>, access: &Rc<Access>) {
    let page = adw::PreferencesPage::new();

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
        let (a, sid, q, id) = ((**access).clone(), data.student_id, data.query.clone(), c.id);
        fill_async(&g, move || api::history(&a, sid, &q, id), |g, attempts| {
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
        let f = |k: &str| e[k].as_str().map(str::to_owned).unwrap_or_else(|| if e[k].is_null() { String::new() } else { e[k].to_string() });
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

    for (id, name) in &c.moodle {
        let g = group(&format!("OLE · {name}"));
        g.set_header_suffix(Some(&link_button("Open", format!("{}/course/view.php?id={id}", auth::OLE))));
        let (a, id, course) = ((**access).clone(), *id, c.name.clone());
        fill_async(&g, move || api::moodle_contents(&a, id), move |g, sections| {
            if sections.is_empty() {
                g.add(&row("No material posted", ""));
            }
            for (name, items) in sections {
                let ex = adw::ExpanderRow::builder().title(glib::markup_escape_text(if name.is_empty() { "General" } else { &name })).subtitle(format!("{} items", items.len())).build();
                for it in &items {
                    ex.add_row(&item_row(it, &course));
                }
                g.add(&ex);
            }
        });
        page.add(&g);
    }

    for (gid, name, url) in &c.teams {
        let g = group(&format!("Teams · {name}"));
        g.set_header_suffix(Some(&link_button("Open in Teams", url.clone())));
        let (a, gid, course) = ((**access).clone(), gid.clone(), c.name.clone());
        fill_async(&g, move || api::team_files(&a, &gid), move |g, files| {
            if files.is_empty() {
                g.add(&row("No files", ""));
            }
            for it in &files {
                g.add(&item_row(it, &course));
            }
        });
        page.add(&g);
    }
    if c.moodle.is_empty() && c.teams.is_empty() {
        let g = group("Material");
        g.set_description(Some("No OLE course or team found. Assign one in ~/.config/unibz-terminal/config.json under \"overrides\"."));
        page.add(&g);
    }

    ui.content.set_title(&c.name);
    ui.content.set_child(Some(&toolbar(&page, None)));
    ui.split.set_show_content(true);
}
