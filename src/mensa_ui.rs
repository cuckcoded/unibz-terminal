//! Mensa page: credit, meals left and the menu for the coming days.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::glib;
use webkit6::prelude::*;

use crate::auth::{self, Res};
use crate::mensa::{self, AREA, BASE, DINNER, Dish, LUNCH, SNS_LOGIN};
use crate::ui::*;

/// Signs in through "SNS Access UNIBZ" in an off-screen browser and returns the reserved area's text.
/// Microsoft usually completes on its own from the stored session; the window only appears if a
/// Microsoft page sits waiting for the user.
async fn login_and_read() -> Res<String> {
    let view = auth::web_view();
    let win = gtk::Window::builder().title("Signing in to the mensa…").default_width(520).default_height(720).child(&view).build();
    auth::set_parent(&win);
    let (tx, rx) = futures_channel::oneshot::channel::<Res<String>>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let send = {
        let tx = tx.clone();
        Rc::new(move |r: Res<String>| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(r);
            }
        })
    };
    // Each step is decided from where we landed, so unexpected redirects don't derail it.
    let (tried_login, landings) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(0)));
    let loads = Rc::new(Cell::new(0u32));
    let weak_win = win.downgrade();
    view.connect_load_changed(move |v, ev| {
        if ev == webkit6::LoadEvent::Started {
            loads.set(loads.get() + 1);
        }
        if ev != webkit6::LoadEvent::Finished {
            return;
        }
        let uri = v.uri().map(|u| u.to_string()).unwrap_or_default();
        if uri.starts_with(AREA) {
            let (v, send) = (v.clone(), send.clone());
            glib::spawn_future_local(async move {
                for attempt in 0..3 {
                    match v.evaluate_javascript_future("document.body.innerText", None, None).await {
                        Ok(j) => {
                            let t = j.to_str().to_string();
                            if mensa::parse_balance(&t).is_some() || attempt == 2 {
                                return send(Ok(t));
                            }
                        }
                        Err(e) => return send(Err(e.to_string())),
                    }
                    glib::timeout_future(Duration::from_millis(800)).await;
                }
            });
        } else if uri.starts_with(&format!("{BASE}/auth")) {
            if tried_login.replace(true) {
                send(Err("the mensa did not accept the SNS sign-in".into()));
            } else {
                v.load_uri(SNS_LOGIN);
            }
        } else if uri.starts_with(BASE) && !uri.contains("/api/") {
            // Signed in and dropped somewhere else on the site.
            landings.set(landings.get() + 1);
            if landings.get() > 3 {
                send(Err("could not reach the reserved area".into()));
            } else {
                v.load_uri(AREA);
            }
        } else {
            // On Microsoft: if nothing moves on for a few seconds, it needs the user.
            let (n, loads, weak_win) = (loads.get(), loads.clone(), weak_win.clone());
            glib::timeout_add_local_once(Duration::from_secs(4), move || {
                if let Some(w) = weak_win.upgrade().filter(|_| loads.get() == n) {
                    w.present();
                }
            });
        }
    });
    win.connect_close_request(move |_| {
        tx.borrow_mut().take();
        glib::Propagation::Proceed
    });
    view.load_uri(AREA);
    let r = glib::future_with_timeout(Duration::from_secs(180), rx).await;
    win.close();
    match r {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("mensa sign-in window closed".into()),
        Err(_) => Err("mensa sign-in timed out".into()),
    }
}

async fn balance_with(cookie: String) -> Res<Option<f64>> {
    if cookie.is_empty() {
        return Ok(None);
    }
    gtk::gio::spawn_blocking(move || mensa::fetch_balance(&cookie)).await.unwrap_or_else(|_| Err("background job panicked".into()))
}

/// Reads the balance with a saved mensa session; signs in (off-screen) only when that session has expired.
async fn fetch_credit() -> Res<f64> {
    // The mensa's session cookie is session-only, so the browser forgets it on exit; keep our own copy.
    if let Some(b) = balance_with(auth::stored("mensa").unwrap_or_default()).await? {
        return Ok(b);
    }
    let text = login_and_read().await?;
    let cookie = auth::cookie_header(AREA).await;
    auth::store("mensa", (!cookie.is_empty()).then_some(cookie));
    mensa::parse_balance(&text).ok_or_else(|| "could not find the balance on the reserved-area page".into())
}

#[derive(Default)]
struct CreditCache {
    value: Option<Res<f64>>,
    loading: bool,
    waiters: Vec<futures_channel::oneshot::Sender<Res<f64>>>,
}

thread_local! {
    static CREDIT: RefCell<CreditCache> = RefCell::default();
}

/// Mensa credit, fetched once per run and shared by Home and the Mensa page; `force` re-reads it.
pub async fn credit(force: bool) -> Res<f64> {
    enum Next {
        Done(Res<f64>),
        Wait(futures_channel::oneshot::Receiver<Res<f64>>),
        Fetch,
    }
    let next = CREDIT.with(|c| {
        let mut c = c.borrow_mut();
        if c.loading {
            let (tx, rx) = futures_channel::oneshot::channel();
            c.waiters.push(tx);
            return Next::Wait(rx);
        }
        match (&c.value, force) {
            (Some(v), false) => Next::Done(v.clone()),
            _ => {
                c.loading = true;
                Next::Fetch
            }
        }
    });
    match next {
        Next::Done(v) => v,
        Next::Wait(rx) => rx.await.unwrap_or(Err("cancelled".into())),
        Next::Fetch => {
            let r = fetch_credit().await;
            let waiters = CREDIT.with(|c| {
                let mut c = c.borrow_mut();
                c.loading = false;
                c.value = Some(r.clone());
                std::mem::take(&mut c.waiters)
            });
            for w in waiters {
                let _ = w.send(r.clone());
            }
            r
        }
    }
}

pub fn euros(v: f64) -> String {
    format!("€ {v:.2}")
}

fn open_reserved_area() {
    let view = auth::web_view();
    let win = gtk::Window::builder().title("Mensa reserved area").default_width(900).default_height(800).child(&view).build();
    auth::set_parent(&win);
    view.load_uri(AREA);
    win.present();
}

fn credit_group() -> adw::PreferencesGroup {
    let g = group("Credit");
    let label = gtk::Label::builder().label("…").css_classes(["title-3"]).build();
    let r = row("Balance", "From the mensa reserved area");
    r.add_suffix(&label);
    g.add(&r);
    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh credit").css_classes(["flat"]).valign(gtk::Align::Center).build();
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.append(&button("Reserved area", |_| open_reserved_area()));
    header.append(&refresh);
    g.set_header_suffix(Some(&header));
    let load = Rc::new({
        let (g, label, refresh) = (g.clone(), label.clone(), refresh.clone());
        move |force: bool| {
            let (g, label, refresh) = (g.clone(), label.clone(), refresh.clone());
            refresh.set_sensitive(false);
            label.set_label("…");
            glib::spawn_future_local(async move {
                match credit(force).await {
                    Ok(v) => {
                        label.set_label(&euros(v));
                        g.set_description(None);
                    }
                    Err(e) => {
                        label.set_label("?");
                        g.set_description(Some(&format!("Could not read the credit: {e}. Use “Reserved area” to check it there.")));
                    }
                }
                refresh.set_sensitive(true);
            });
        }
    });
    refresh.connect_clicked({
        let load = load.clone();
        move |_| load(true)
    });
    load(false);
    g
}

fn day_label(offset: i32, date: &glib::DateTime) -> String {
    let d = date.format("%A %d %B").map(|s| s.to_string()).unwrap_or_default();
    match offset {
        0 => format!("Today · {d}"),
        1 => format!("Tomorrow · {d}"),
        -1 => format!("Yesterday · {d}"),
        _ => d,
    }
}

fn dish_card(d: &Dish) -> gtk::Box {
    let card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["card"]).overflow(gtk::Overflow::Hidden).build();
    let pic = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).height_request(130).can_shrink(true).build();
    card.append(&pic);
    if !d.image.is_empty() {
        let (url, pic2) = (d.image.clone(), pic.clone());
        spawn(move || mensa::image(&url), move |r| {
            if let Ok(path) = r {
                pic2.set_filename(Some(path));
            }
        });
    }
    let name = gtk::Label::builder().label(&d.name).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).xalign(0.0).margin_top(8).margin_bottom(10).margin_start(10).margin_end(10).build();
    card.append(&name);
    card.set_tooltip_text(Some(&if d.allergens.is_empty() { d.name.clone() } else { format!("{}\n\nAllergens: {}", d.name, d.allergens) }));
    if !d.url.is_empty() {
        let click = gtk::GestureClick::new();
        let url = d.url.clone();
        click.connect_released(move |g, _, _, _| {
            if let Some(w) = g.widget() {
                open_uri(&w, &url);
            }
        });
        card.add_controller(click);
        card.set_cursor_from_name(Some("pointer"));
    }
    card
}

/// One column per dish type (first courses, second courses, sides, …), in the site's order.
fn columns(dishes: &[Dish]) -> gtk::Widget {
    if dishes.is_empty() {
        return adw::StatusPage::builder().icon_name("emoji-food-symbolic").title("No menu").description("Nothing is published for this meal.").build().upcast();
    }
    let mut cats: Vec<(&str, Vec<&Dish>)> = Vec::new();
    for d in dishes {
        match cats.iter_mut().find(|(c, _)| *c == d.category) {
            Some((_, v)) => v.push(d),
            None => cats.push((&d.category, vec![d])),
        }
    }
    let cols = gtk::Box::builder().spacing(18).homogeneous(true).build();
    for (cat, ds) in cats {
        let col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).width_request(180).build();
        col.append(&gtk::Label::builder().label(if cat.is_empty() { "Other" } else { cat }).css_classes(["heading"]).xalign(0.0).build());
        for d in ds {
            col.append(&dish_card(d));
        }
        cols.append(&col);
    }
    cols.upcast()
}

pub fn page() -> gtk::Widget {
    let outer = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(18).margin_bottom(18).margin_start(18).margin_end(18).build();
    outer.append(&adw::Clamp::builder().maximum_size(800).child(&credit_group()).build());

    // Day navigation, meal and location.
    let bar = gtk::Box::builder().spacing(6).build();
    let prev = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Previous day").build();
    let today = gtk::Button::builder().label("Today").build();
    let next = gtk::Button::builder().icon_name("go-next-symbolic").tooltip_text("Next day").build();
    let nav = gtk::Box::builder().css_classes(["linked"]).build();
    nav.append(&prev);
    nav.append(&today);
    nav.append(&next);
    let title = gtk::Label::builder().css_classes(["title-3"]).hexpand(true).xalign(0.0).margin_start(12).build();
    let meal = gtk::DropDown::from_strings(&["Lunch", "Dinner"]);
    // After lunch time, dinner is the next meal.
    meal.set_selected(glib::DateTime::now_local().map(|d| (d.hour() >= 15) as u32).unwrap_or(0));
    let location = gtk::DropDown::from_strings(&mensa::LOCATIONS.map(|l| l.1));
    let current = auth::config().mensa_location.unwrap_or(mensa::LOCATIONS[0].0.into());
    location.set_selected(mensa::LOCATIONS.iter().position(|l| l.0 == current).unwrap_or(0) as u32);
    bar.append(&nav);
    bar.append(&title);
    bar.append(&meal);
    bar.append(&location);
    outer.append(&bar);

    let holder = adw::Bin::builder().vexpand(true).build();
    outer.append(&holder);

    let offset = Rc::new(Cell::new(0i32));
    let generation = Rc::new(Cell::new(0u32));
    let cache: Rc<RefCell<HashMap<(String, u8, String), Vec<Dish>>>> = Rc::default();
    let reload: Rc<dyn Fn()> = Rc::new({
        let (title, meal, location, holder, offset) = (title.clone(), meal.clone(), location.clone(), holder.clone(), offset.clone());
        move || {
            let Ok(date) = glib::DateTime::now_local().and_then(|d| d.add_days(offset.get())) else { return };
            title.set_label(&day_label(offset.get(), &date));
            let loc = mensa::LOCATIONS[location.selected() as usize % mensa::LOCATIONS.len()].0.to_owned();
            let m = if meal.selected() == 1 { DINNER } else { LUNCH };
            let key = (loc.clone(), m, date.format("%F").map(|s| s.to_string()).unwrap_or_default());
            if let Some(d) = cache.borrow().get(&key) {
                holder.set_child(Some(&columns(d)));
                return;
            }
            holder.set_child(Some(&adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Start).build()));
            // Ignore answers for days the user has already clicked past.
            generation.set(generation.get() + 1);
            let (gen_now, generation, holder, cache) = (generation.get(), generation.clone(), holder.clone(), cache.clone());
            spawn(move || mensa::day(&loc, m, &date), move |r| {
                if generation.get() != gen_now {
                    return;
                }
                match r {
                    Ok(d) => {
                        holder.set_child(Some(&columns(&d)));
                        cache.borrow_mut().insert(key, d);
                    }
                    Err(e) => holder.set_child(Some(&adw::StatusPage::builder().title("Could not load the menu").description(esc(&e)).build())),
                }
            });
        }
    });
    for (b, delta) in [(&prev, -1), (&today, 0), (&next, 1)] {
        let (offset, reload) = (offset.clone(), reload.clone());
        b.connect_clicked(move |_| {
            offset.set(if delta == 0 { 0 } else { offset.get() + delta });
            reload();
        });
    }
    meal.connect_selected_notify({
        let reload = reload.clone();
        move |_| reload()
    });
    location.connect_selected_notify({
        let reload = reload.clone();
        move |l| {
            let mut cfg = auth::config();
            cfg.mensa_location = Some(mensa::LOCATIONS[l.selected() as usize % mensa::LOCATIONS.len()].0.into());
            if let Err(e) = auth::save_config(&cfg) {
                eprintln!("saving config: {e}");
            }
            reload();
        }
    });
    reload();
    gtk::ScrolledWindow::builder().child(&outer).build().upcast()
}
