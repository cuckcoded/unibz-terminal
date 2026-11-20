//! Calendar: mySNS lectures and exams and OLE events by day, week, month or year.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

use adw::prelude::*;
use gtk::glib;

use crate::api::{self, Data, arr, s};
use crate::auth::{OLE, Res};
use crate::moodle;
use crate::ui::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Lecture,
    Exam,
    Ole,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Lecture, Kind::Exam, Kind::Ole];
    fn css(self) -> &'static str {
        match self {
            Kind::Lecture => "cal-lecture",
            Kind::Exam => "cal-exam",
            Kind::Ole => "cal-ole",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Lecture => "Lectures",
            Kind::Exam => "Exams",
            Kind::Ole => "OLE",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub start: i64,
    /// Equal to `start` for deadlines.
    pub end: i64,
    pub all_day: bool,
    pub title: String,
    pub detail: String,
    pub place: String,
    pub people: String,
    pub kind: Kind,
    /// Sidebar key ("c:12") or a URL to open.
    pub target: String,
}

fn unix(iso: &str) -> Option<i64> {
    glib::DateTime::from_iso8601(iso, Some(&glib::TimeZone::local())).ok().map(|d| d.to_unix())
}

fn join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" · ")
}

/// Lectures and exams from the mySNS timetable, plus exam sessions the timetable doesn't list.
pub fn sns_events(data: &Data) -> Vec<Event> {
    let mut out = Vec::new();
    for c in &data.courses {
        let key = format!("c:{}", c.id);
        for l in &c.lectures {
            let (Some(start), Some(end)) = (unix(&l.start), unix(&l.end)) else { continue };
            let exam = l.kind == "EXAM";
            let title = if exam { format!("Exam · {}", c.name) } else { c.name.clone() };
            out.push(Event { start, end, all_day: false, title, detail: l.activity.clone(), place: l.room.clone(), people: l.staff.clone(), kind: if exam { Kind::Exam } else { Kind::Lecture }, target: key.clone() });
        }
        for e in &c.exams {
            let Some(date) = s(&e["examDate"]).get(..10).map(str::to_owned) else { continue };
            if c.lectures.iter().any(|l| l.kind == "EXAM" && l.start.starts_with(&date)) {
                continue;
            }
            let time = s(&e["examTime"]);
            let hm = time.get(..5).filter(|t| t.contains(':'));
            let Some(start) = unix(&format!("{date}T{}:00", hm.unwrap_or("00:00"))) else { continue };
            let enrolled = if e["enrolmentDone"] == true { "registered" } else { "" };
            out.push(Event {
                start,
                end: start,
                all_day: hm.is_none(),
                title: format!("Exam · {}", c.name),
                detail: join(&[&s(&e["examTypeDescription"]), enrolled]),
                place: s(&e["location"]),
                people: s(&e["examProfessorName"]),
                kind: Kind::Exam,
                target: key.clone(),
            });
        }
    }
    out
}

/// Sidebar key for an OLE course id: the mySNS course it's linked to, else its own entry.
fn key_for(data: &Data, ole: Option<i64>) -> String {
    data.courses
        .iter()
        .find(|c| ole.is_some_and(|o| c.moodle.iter().any(|m| m.0 == o)))
        .map(|c| format!("c:{}", c.id))
        .or_else(|| ole.map(|o| format!("o:{o}")))
        .unwrap_or_default()
}

/// All OLE calendar events (deadlines, course and personal events) between two unix times.
fn ole_events(token: &str, courses: &[(i64, String, String)], from: i64, to: i64) -> Res<Vec<Event>> {
    let names: Vec<String> = (0..courses.len()).map(|i| format!("events[courseids][{i}]")).collect();
    let mut args = vec![("options[timestart]", from.to_string()), ("options[timeend]", to.to_string()), ("options[userevents]", "1".into()), ("options[siteevents]", "1".into())];
    args.extend(names.iter().zip(courses).map(|(n, c)| (n.as_str(), c.0.to_string())));
    let v = moodle::call(token, "core_calendar_get_calendar_events", &args)?;
    Ok(arr(&v["events"])
        .iter()
        .map(|e| {
            let start = e["timestart"].as_i64().unwrap_or_default();
            let course = courses.iter().find(|c| Some(c.0) == e["courseid"].as_i64());
            Event {
                start,
                end: start + e["timeduration"].as_i64().unwrap_or_default(),
                all_day: false,
                title: s(&e["name"]),
                detail: course.map(|c| c.1.clone()).unwrap_or_default(),
                place: String::new(),
                people: String::new(),
                kind: Kind::Ole,
                target: course.map(|c| c.2.clone()).unwrap_or_else(|| format!("{OLE}/calendar/view.php?view=day&time={start}")),
            }
        })
        .collect())
}

fn day_key(d: &glib::DateTime) -> String {
    d.format("%F").map(|s| s.to_string()).unwrap_or_default()
}

fn midnight(d: &glib::DateTime) -> glib::DateTime {
    let (y, m, day) = d.ymd();
    glib::DateTime::from_local(y, m, day, 0, 0, 0.0).unwrap_or_else(|_| d.clone())
}

fn fmt(d: &glib::DateTime, f: &str) -> String {
    d.format(f).map(|s| s.to_string()).unwrap_or_default()
}

fn hm(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts).map(|d| fmt(&d, "%H:%M")).unwrap_or_default()
}

fn when(e: &Event) -> String {
    if e.all_day {
        "All day".into()
    } else if e.end > e.start {
        format!("{}–{}", hm(e.start), hm(e.end))
    } else {
        hm(e.start)
    }
}

/// Events grouped by local day ("YYYY-MM-DD"), each day sorted by time.
fn index(events: &[Event]) -> HashMap<String, Vec<Event>> {
    let mut by_day: HashMap<String, Vec<Event>> = HashMap::new();
    for e in events {
        if let Ok(d) = glib::DateTime::from_unix_local(e.start) {
            by_day.entry(day_key(&d)).or_default().push(e.clone());
        }
    }
    for v in by_day.values_mut() {
        v.sort_by_key(|e| (!e.all_day, e.start));
    }
    by_day
}

const CSS: &str = "
.cal-dot { min-width: 10px; min-height: 10px; border-radius: 5px; }
.cal-dot.cal-lecture { background-color: @accent_bg_color; }
.cal-dot.cal-exam { background-color: @error_bg_color; }
.cal-dot.cal-ole { background-color: @warning_bg_color; }
.cal-chip { border-radius: 6px; padding: 2px 6px; font-size: 0.85em; min-height: 0; }
.cal-chip.cal-lecture { background-color: alpha(@accent_bg_color, 0.2); }
.cal-chip.cal-exam { background-color: alpha(@error_bg_color, 0.25); }
.cal-chip.cal-ole { background-color: alpha(@warning_bg_color, 0.3); }
.cal-cell { padding: 4px; border-radius: 8px; }
.cal-today { color: @accent_color; font-weight: bold; }
.cal-day { min-width: 26px; min-height: 26px; padding: 0; font-size: 0.85em; }
.cal-busy { background-color: alpha(@accent_bg_color, 0.2); }
.cal-lunch { background-color: alpha(@success_bg_color, 0.15); border: 1px dashed alpha(@success_bg_color, 0.7); border-radius: 6px; padding: 2px 8px; min-height: 0; font-size: 0.85em; }
.cal-event { border-radius: 10px; padding: 8px 10px; border-left: 4px solid; font-weight: normal; }
.cal-event.cal-lecture { border-left-color: @accent_bg_color; background-color: alpha(@accent_bg_color, 0.12); }
.cal-event.cal-exam { border-left-color: @error_bg_color; background-color: alpha(@error_bg_color, 0.14); }
.cal-event.cal-ole { border-left-color: @warning_bg_color; background-color: alpha(@warning_bg_color, 0.16); }
.cal-event .cal-title { font-weight: bold; }
";

fn install_css() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let p = gtk::CssProvider::new();
        p.load_from_string(CSS);
        if let Some(d) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
}

fn dot(kind: Kind) -> gtk::Box {
    gtk::Box::builder().css_classes(["cal-dot", kind.css()]).valign(gtk::Align::Center).build()
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Day,
    Week,
    Month,
    Year,
}

struct Cal {
    events: RefCell<Vec<Event>>,
    by_day: RefCell<HashMap<String, Vec<Event>>>,
    anchor: RefCell<glib::DateTime>,
    view: Cell<View>,
    title: gtk::Label,
    holder: adw::Bin,
    toggles: Vec<(View, gtk::ToggleButton)>,
    open: Rc<dyn Fn(&str)>,
}

impl Cal {
    fn add(&self, more: Vec<Event>) {
        self.events.borrow_mut().extend(more);
        *self.by_day.borrow_mut() = index(&self.events.borrow());
    }

    fn on(&self, d: &glib::DateTime) -> Vec<Event> {
        self.by_day.borrow().get(&day_key(d)).cloned().unwrap_or_default()
    }

    fn activate(&self, w: &impl IsA<gtk::Widget>, target: &str) {
        if target.starts_with("http") {
            open_uri(w, target);
        } else if !target.is_empty() {
            (self.open)(target);
        }
    }

    fn go(self: &Rc<Self>, view: View, day: glib::DateTime) {
        self.view.set(view);
        *self.anchor.borrow_mut() = day;
        self.render();
    }

    fn step(self: &Rc<Self>, delta: i32) {
        let a = self.anchor.borrow().clone();
        let next = match self.view.get() {
            View::Day => a.add_days(delta),
            View::Week => a.add_days(7 * delta),
            View::Month => a.add_months(delta),
            View::Year => a.add_years(delta),
        };
        if let Ok(n) = next {
            self.go(self.view.get(), n);
        }
    }

    fn render(self: &Rc<Self>) {
        for (v, t) in &self.toggles {
            t.set_active(*v == self.view.get());
        }
        let a = self.anchor.borrow().clone();
        let (title, child) = match self.view.get() {
            View::Day => (fmt(&a, "%A %d %B %Y"), self.day_view(&a)),
            View::Week => {
                let start = a.add_days(1 - a.day_of_week()).unwrap_or(a.clone());
                let end = start.add_days(6).unwrap_or(start.clone());
                (format!("Week {} · {} – {}", fmt(&start, "%V"), fmt(&start, "%d %b"), fmt(&end, "%d %b %Y")), self.week_view(&start))
            }
            View::Month => (fmt(&a, "%B %Y"), self.month_view(&a)),
            View::Year => (fmt(&a, "%Y"), self.year_view(&a)),
        };
        self.title.set_label(&title);
        self.holder.set_child(Some(&child));
    }

    /// Timetable card: time, course, then type, room and lecturer on their own lines.
    fn card(self: &Rc<Self>, e: &Event) -> gtk::Button {
        let label = |text: &str, classes: &[&str]| {
            gtk::Label::builder().label(text).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).css_classes(classes.to_vec()).build()
        };
        let v = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).build();
        v.append(&label(&when(e), &["caption", "dim-label"]));
        v.append(&label(&e.title, &["cal-title"]));
        if !e.detail.is_empty() {
            v.append(&label(&e.detail, &["caption"]));
        }
        for (icon, text) in [("mark-location-symbolic", &e.place), ("system-users-symbolic", &e.people)] {
            if !text.is_empty() {
                let line = gtk::Box::builder().spacing(6).build();
                line.append(&gtk::Image::builder().icon_name(icon).pixel_size(12).css_classes(["dim-label"]).build());
                line.append(&label(text, &["caption", "dim-label"]));
                v.append(&line);
            }
        }
        let b = gtk::Button::builder().child(&v).css_classes(["cal-event", e.kind.css()]).tooltip_text(join(&[&when(e), &e.title, &e.detail, &e.place, &e.people])).build();
        let (weak, target) = (Rc::downgrade(self), e.target.clone());
        b.connect_clicked(move |b| {
            if let Some(c) = weak.upgrade() {
                c.activate(b, &target);
            }
        });
        b
    }

    fn lunch_bar(self: &Rc<Self>) -> gtk::Button {
        let content = gtk::Box::builder().spacing(6).build();
        content.append(&gtk::Image::from_icon_name("emoji-food-symbolic"));
        let hm = |m: i64| format!("{:02}:{:02}", m / 60, m % 60);
        content.append(&gtk::Label::new(Some(&format!("Lunch · {}–{}", hm(LUNCH.0), hm(LUNCH.1)))));
        let b = gtk::Button::builder().child(&content).css_classes(["cal-lunch"]).tooltip_text("Open the mensa menu").hexpand(true).build();
        let weak = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(c) = weak.upgrade() {
                (c.open)("m:mensa");
            }
        });
        b
    }

    fn day_view(self: &Rc<Self>, day: &glib::DateTime) -> gtk::Widget {
        let evs = self.on(day);
        if evs.is_empty() {
            return adw::StatusPage::builder().icon_name("x-office-calendar-symbolic").title("Nothing on this day").build().upcast();
        }
        let (morning, afternoon) = split_at_lunch(day, evs);
        let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).valign(gtk::Align::Start).build();
        for e in &morning {
            list.append(&self.card(e));
        }
        list.append(&self.lunch_bar());
        for e in &afternoon {
            list.append(&self.card(e));
        }
        scrolled(&adw::Clamp::builder().maximum_size(700).child(&list).margin_top(6).margin_bottom(18).margin_start(12).margin_end(12).build())
    }

    fn day_button(self: &Rc<Self>, child: &impl IsA<gtk::Widget>, day: &glib::DateTime) -> gtk::Button {
        let b = gtk::Button::builder().child(child).css_classes(["flat"]).build();
        let (weak, day) = (Rc::downgrade(self), day.clone());
        b.connect_clicked(move |_| {
            if let Some(c) = weak.upgrade() {
                c.go(View::Day, day.clone());
            }
        });
        b
    }

    fn week_view(self: &Rc<Self>, start: &glib::DateTime) -> gtk::Widget {
        let grid = gtk::Grid::builder().column_homogeneous(true).column_spacing(10).row_spacing(10).margin_start(12).margin_end(12).margin_bottom(18).build();
        let today = today_key();
        for i in 0..7 {
            let Ok(d) = start.add_days(i) else { continue };
            let head = gtk::Label::builder().label(fmt(&d, "%a %d")).css_classes(["heading"]).build();
            if day_key(&d) == today {
                head.add_css_class("cal-today");
            }
            grid.attach(&self.day_button(&head, &d), i, 0, 1, 1);
            let (morning, afternoon) = split_at_lunch(&d, self.on(&d));
            for (row, evs) in [(1, morning), (3, afternoon)] {
                let col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).width_request(150).valign(if row == 1 { gtk::Align::End } else { gtk::Align::Start }).build();
                for e in &evs {
                    col.append(&self.card(e));
                }
                grid.attach(&col, i, row, 1, 1);
            }
        }
        grid.attach(&self.lunch_bar(), 0, 2, 7, 1);
        // Narrow windows scroll sideways instead of squeezing the cards.
        let sw = gtk::ScrolledWindow::builder().child(&grid).vexpand(true).build();
        sw.upcast()
    }

    fn month_view(self: &Rc<Self>, a: &glib::DateTime) -> gtk::Widget {
        let first = glib::DateTime::from_local(a.year(), a.month(), 1, 0, 0, 0.0).unwrap_or(a.clone());
        let start = first.add_days(1 - first.day_of_week()).unwrap_or(first.clone());
        let grid = gtk::Grid::builder().column_homogeneous(true).row_homogeneous(false).column_spacing(4).row_spacing(4).margin_start(12).margin_end(12).margin_bottom(18).build();
        for (i, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
            grid.attach(&gtk::Label::builder().label(*name).css_classes(["dim-label", "caption-heading"]).build(), i as i32, 0, 1, 1);
        }
        let today = today_key();
        for cell in 0..42 {
            let Ok(d) = start.add_days(cell) else { continue };
            if cell % 7 == 0 && cell > 0 && d.month() != a.month() {
                break;
            }
            let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).height_request(96).build();
            let num = gtk::Label::builder().label(d.day_of_month().to_string()).xalign(0.0).build();
            if day_key(&d) == today {
                num.add_css_class("cal-today");
            } else if d.month() != a.month() {
                num.add_css_class("dim-label");
            }
            b.append(&num);
            let evs = self.on(&d);
            for e in evs.iter().take(3) {
                b.append(&gtk::Label::builder().label(format!("{} {}", when(e), e.title)).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["cal-chip", e.kind.css()]).build());
            }
            if evs.len() > 3 {
                b.append(&gtk::Label::builder().label(format!("+{} more", evs.len() - 3)).xalign(0.0).css_classes(["caption", "dim-label"]).build());
            }
            let btn = self.day_button(&b, &d);
            btn.add_css_class("cal-cell");
            let tip = evs.iter().map(|e| format!("{} {}", when(e), e.title)).collect::<Vec<_>>().join("\n");
            btn.set_tooltip_text((!tip.is_empty()).then_some(tip.as_str()));
            grid.attach(&btn, cell % 7, 1 + cell / 7, 1, 1);
        }
        scrolled(&grid)
    }

    fn year_view(self: &Rc<Self>, a: &glib::DateTime) -> gtk::Widget {
        let flow = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(true).min_children_per_line(1).max_children_per_line(4).column_spacing(18).row_spacing(18).valign(gtk::Align::Start).margin_start(12).margin_end(12).margin_bottom(18).build();
        let today = today_key();
        for m in 1..=12 {
            let Ok(first) = glib::DateTime::from_local(a.year(), m, 1, 0, 0, 0.0) else { continue };
            let bx = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
            let head = gtk::Button::builder().label(fmt(&first, "%B")).css_classes(["flat", "heading"]).halign(gtk::Align::Start).build();
            let weak = Rc::downgrade(self);
            let f = first.clone();
            head.connect_clicked(move |_| {
                if let Some(c) = weak.upgrade() {
                    c.go(View::Month, f.clone());
                }
            });
            bx.append(&head);
            let grid = gtk::Grid::builder().column_homogeneous(true).build();
            for (i, n) in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate() {
                grid.attach(&gtk::Label::builder().label(*n).css_classes(["dim-label", "caption"]).build(), i as i32, 0, 1, 1);
            }
            let offset = first.day_of_week() - 1;
            let mut d = first.clone();
            while d.month() == m {
                let pos = offset + d.day_of_month() - 1;
                let btn = self.day_button(&gtk::Label::new(Some(&d.day_of_month().to_string())), &d);
                btn.add_css_class("cal-day");
                let n = self.on(&d).len();
                if n > 0 {
                    btn.add_css_class("cal-busy");
                    btn.set_tooltip_text(Some(&format!("{n} event{}", if n == 1 { "" } else { "s" })));
                }
                if day_key(&d) == today {
                    btn.add_css_class("cal-today");
                }
                grid.attach(&btn, pos % 7, 1 + pos / 7, 1, 1);
                let Ok(next) = d.add_days(1) else { break };
                d = next;
            }
            bx.append(&grid);
            flow.append(&bx);
        }
        scrolled(&flow)
    }
}

/// Mensa lunch service, (start, end) in minutes after midnight.
const LUNCH: (i64, i64) = (12 * 60, 13 * 60 + 30);

/// A day's events before lunch (including all-day ones) and from lunch on.
fn split_at_lunch(day: &glib::DateTime, evs: Vec<Event>) -> (Vec<Event>, Vec<Event>) {
    let lunch = midnight(day).to_unix() + LUNCH.0 * 60;
    evs.into_iter().partition(|e| e.all_day || e.start < lunch)
}

fn today_key() -> String {
    glib::DateTime::now_local().map(|d| day_key(&d)).unwrap_or_default()
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    gtk::ScrolledWindow::builder().child(child).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build().upcast()
}

/// `open` selects a sidebar entry.
pub fn page(data: &Data, access: &api::Access, open: Rc<dyn Fn(&str)>) -> gtk::Widget {
    install_css();
    let outer = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();

    // Toolbar: previous/today/next, title, view switcher.
    let bar = gtk::Box::builder().spacing(6).margin_top(12).margin_start(12).margin_end(12).build();
    let nav = gtk::Box::builder().css_classes(["linked"]).build();
    let prev = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Previous").build();
    let today = gtk::Button::builder().label("Today").build();
    let next = gtk::Button::builder().icon_name("go-next-symbolic").tooltip_text("Next").build();
    for b in [&prev, &today, &next] {
        nav.append(b);
    }
    let title = gtk::Label::builder().css_classes(["title-3"]).hexpand(true).xalign(0.0).margin_start(12).ellipsize(gtk::pango::EllipsizeMode::End).build();
    let views = gtk::Box::builder().css_classes(["linked"]).build();
    let mut toggles: Vec<(View, gtk::ToggleButton)> = Vec::new();
    for (v, name) in [(View::Day, "Day"), (View::Week, "Week"), (View::Month, "Month"), (View::Year, "Year")] {
        let t = gtk::ToggleButton::builder().label(name).build();
        if let Some((_, first)) = toggles.first() {
            t.set_group(Some(first));
        }
        views.append(&t);
        toggles.push((v, t));
    }
    bar.append(&nav);
    bar.append(&title);
    bar.append(&views);
    outer.append(&bar);

    // Legend and load problems.
    let legend = gtk::Box::builder().spacing(14).margin_start(12).margin_end(12).build();
    for k in Kind::ALL {
        let item = gtk::Box::builder().spacing(6).build();
        item.append(&dot(k));
        item.append(&gtk::Label::builder().label(k.label()).css_classes(["caption"]).build());
        legend.append(&item);
    }
    let status = gtk::Label::builder().css_classes(["caption", "dim-label"]).hexpand(true).xalign(1.0).wrap(true).build();
    legend.append(&status);
    outer.append(&legend);

    let holder = adw::Bin::builder().vexpand(true).build();
    outer.append(&holder);

    let now = glib::DateTime::now_local().map(|d| midnight(&d)).unwrap_or_else(|_| glib::DateTime::from_unix_utc(0).unwrap());
    let cal = Rc::new(Cal { events: RefCell::default(), by_day: RefCell::default(), anchor: RefCell::new(now.clone()), view: Cell::new(View::Week), title, holder, toggles, open });
    cal.add(sns_events(data));
    cal.render();

    let weak: Weak<Cal> = Rc::downgrade(&cal);
    for (v, t) in &cal.toggles {
        let (weak, v) = (weak.clone(), *v);
        t.connect_toggled(move |t| {
            if let Some(c) = weak.upgrade().filter(|c| t.is_active() && c.view.get() != v) {
                let a = c.anchor.borrow().clone();
                c.go(v, a);
            }
        });
    }
    for (b, delta) in [(&prev, -1), (&next, 1)] {
        let weak = weak.clone();
        b.connect_clicked(move |_| {
            if let Some(c) = weak.upgrade() {
                c.step(delta);
            }
        });
    }
    // This closure owns the calendar state, so it lives exactly as long as the page; everything else holds it weakly.
    let keep = cal.clone();
    today.connect_clicked(move |_| {
        if let Ok(d) = glib::DateTime::now_local() {
            keep.go(keep.view.get(), midnight(&d));
        }
    });

    // OLE loads in the background over the academic year (Sep 1 to Sep 1).
    let year = if now.month() >= 9 { now.year() } else { now.year() - 1 };
    let from = glib::DateTime::from_local(year, 9, 1, 0, 0, 0.0).map(|d| d.to_unix()).unwrap_or_default();
    let to = from + 366 * 86400;
    let problems: Rc<RefCell<Vec<String>>> = Rc::default();
    let report = {
        let (weak, status, problems) = (weak.clone(), status.clone(), problems.clone());
        move |what: &str, r: Res<Vec<Event>>| {
            let Some(c) = weak.upgrade() else { return };
            match r {
                Ok(evs) => {
                    c.add(evs);
                    c.render();
                }
                Err(e) => problems.borrow_mut().push(format!("{what} unavailable: {e}")),
            }
            status.set_label(&problems.borrow().join("\n"));
        }
    };
    match &access.moodle {
        Ok(token) => {
            let token = token.clone();
            let courses: Vec<(i64, String, String)> = data.ole.iter().map(|(id, name)| (*id, name.clone(), key_for(data, Some(*id)))).collect();
            let report = report.clone();
            spawn(move || ole_events(&token, &courses, from, to), move |r| report("OLE", r));
        }
        Err(e) => report("OLE", Err(e.clone())),
    }
    outer.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_groups_by_local_day_and_sorts() {
        let t = |iso: &str| unix(iso).unwrap();
        let ev = |start, all_day| Event { start, end: start, all_day, title: String::new(), detail: String::new(), place: String::new(), people: String::new(), kind: Kind::Ole, target: String::new() };
        let idx = index(&[ev(t("2026-11-20T14:00:00"), false), ev(t("2026-11-20T09:00:00"), false), ev(t("2026-11-20T00:00:00"), true), ev(t("2026-11-21T08:00:00"), false)]);
        let day = &idx["2026-11-20"];
        assert_eq!(day.len(), 3);
        assert!(day[0].all_day);
        assert_eq!(hm(day[1].start), "09:00");
        assert_eq!(idx["2026-11-21"].len(), 1);
        // All-day and morning events come before lunch, 12:00 on after it.
        let fri = glib::DateTime::from_local(2026, 11, 20, 0, 0, 0.0).unwrap();
        let (am, pm) = split_at_lunch(&fri, day.clone());
        assert_eq!((am.len(), pm.len()), (2, 1));
    }
}
