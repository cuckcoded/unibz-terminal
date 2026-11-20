//! Mensa page: credit, meals left and the menu for the coming days.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::glib;
use webkit6::prelude::*;

use crate::auth::{self, Res};
use crate::mensa::{self, AREA, BASE, DayMenu, Dish, SNS_LOGIN};
use crate::ui::*;

/// Visible text of the reserved area, signing in with SNS when needed. The window only shows
/// up if Microsoft needs interaction; otherwise everything happens in the background.
async fn account_text() -> Res<String> {
    let view = auth::web_view();
    let win = gtk::Window::builder().title("Sign in to the mensa").default_width(520).default_height(720).modal(true).child(&view).build();
    auth::set_parent(&win);
    let (tx, rx) = futures_channel::oneshot::channel::<Res<String>>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    // 0 = setting language, 1 = reserved area requested, 2 = SNS login started, 3 = reserved area after login, 4 = reading.
    let stage = Rc::new(Cell::new(0u8));
    let (weak, tx2) = (win.downgrade(), tx.clone());
    let finish = Rc::new(move |r: Res<String>| {
        if let Some(tx) = tx2.borrow_mut().take() {
            let _ = tx.send(r);
        }
        if let Some(w) = weak.upgrade() {
            w.close();
        }
    });
    let weak = win.downgrade();
    view.connect_load_changed(move |v, ev| {
        if ev != webkit6::LoadEvent::Finished {
            return;
        }
        let uri = v.uri().map(|u| u.to_string()).unwrap_or_default();
        let login_page = uri.starts_with(&format!("{BASE}/auth"));
        match stage.get() {
            0 => {
                stage.set(1);
                v.load_uri(AREA);
            }
            4 => {}
            s if login_page => {
                if s == 1 {
                    stage.set(2);
                    v.load_uri(SNS_LOGIN);
                } else {
                    finish(Err("Mensa login did not succeed".into()));
                }
            }
            // Microsoft sign-in: show the window in case it needs the user.
            _ if !uri.starts_with(BASE) => {
                if let Some(w) = weak.upgrade() {
                    w.present();
                }
            }
            2 if !uri.starts_with(AREA) => {
                stage.set(3);
                v.load_uri(AREA);
            }
            _ => {
                stage.set(4);
                let (v, finish) = (v.clone(), finish.clone());
                glib::spawn_future_local(async move {
                    // The page fills in parts with scripts; give it a few tries.
                    for attempt in 0..6 {
                        glib::timeout_future(Duration::from_millis(1500)).await;
                        let text = v.evaluate_javascript_future("document.body.innerText", None, None).await.map(|j| j.to_str().to_string());
                        match text {
                            Ok(t) if mensa::parse_account(&t).0.is_some() || attempt == 5 => return finish(Ok(t)),
                            Ok(_) => {}
                            Err(e) => return finish(Err(e.to_string())),
                        }
                    }
                });
            }
        }
    });
    win.connect_close_request(move |_| {
        tx.borrow_mut().take();
        glib::Propagation::Proceed
    });
    view.load_uri(&format!("{BASE}/change_language/en"));
    let r = glib::future_with_timeout(Duration::from_secs(120), rx).await;
    win.close();
    match r {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("mensa sign-in window closed".into()),
        Err(_) => Err("mensa sign-in timed out".into()),
    }
}

fn open_reserved_area() {
    let view = auth::web_view();
    let win = gtk::Window::builder().title("Mensa reserved area").default_width(900).default_height(800).child(&view).build();
    auth::set_parent(&win);
    view.load_uri(AREA);
    win.present();
}

fn big_label() -> gtk::Label {
    gtk::Label::builder().label("…").css_classes(["title-3"]).build()
}

fn day_title(d: &DayMenu) -> String {
    let date = glib::DateTime::from_unix_local(d.unix).ok().and_then(|t| t.format("%A %d %B").ok()).map_or(d.date.clone(), |s| s.to_string());
    let day = |offset| glib::DateTime::now_local().and_then(|t| t.add_days(offset)).ok().and_then(|t| t.format("%d/%m/%Y").ok()).map(|s| s.to_string());
    if day(0).as_deref() == Some(d.date.as_str()) {
        format!("Today · {date}")
    } else if day(1).as_deref() == Some(d.date.as_str()) {
        format!("Tomorrow · {date}")
    } else {
        date
    }
}

fn meal_row(title: &str, dishes: &[Dish], expanded: bool) -> adw::ExpanderRow {
    let ex = adw::ExpanderRow::builder().title(title).subtitle(format!("{} dishes", dishes.len())).expanded(expanded).build();
    let mut by_cat: Vec<(&str, Vec<&Dish>)> = Vec::new();
    for d in dishes {
        match by_cat.iter_mut().find(|(c, _)| *c == d.category) {
            Some((_, v)) => v.push(d),
            None => by_cat.push((&d.category, vec![d])),
        }
    }
    for (cat, ds) in by_cat {
        for d in ds {
            let sub = [cat.to_owned(), d.allergens.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            let r = row(&d.name, &sub);
            r.set_subtitle_lines(2);
            if !d.url.is_empty() {
                r.set_activatable(true);
                r.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
                let url = d.url.clone();
                r.connect_activated(move |r| open_uri(r, &url));
            }
            ex.add_row(&r);
        }
    }
    ex
}

pub fn page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let cfg = auth::config();

    // Credit.
    let credit = group("Credit");
    let (balance_label, meals_label) = (big_label(), big_label());
    let balance_row = row("Balance", "From the mensa reserved area");
    balance_row.add_suffix(&balance_label);
    let price = adw::SpinRow::builder().title("Price per meal (€)").digits(2).adjustment(&gtk::Adjustment::new(cfg.mensa_meal_price.unwrap_or(0.0), 0.0, 30.0, 0.05, 0.5, 0.0)).build();
    let meals_row = row("Meals you can afford", "Balance ÷ price per meal");
    meals_row.add_suffix(&meals_label);
    credit.add(&balance_row);
    credit.add(&price);
    credit.add(&meals_row);

    let balance: Rc<Cell<Option<f64>>> = Rc::default();
    let update_meals: Rc<dyn Fn()> = Rc::new({
        let (balance, price, meals_label) = (balance.clone(), price.clone(), meals_label.clone());
        move || {
            meals_label.set_label(&match (balance.get(), price.value()) {
                (Some(b), p) if p > 0.0 => ((b / p).floor().max(0.0) as i64).to_string(),
                (Some(_), _) => "set a price".into(),
                (None, _) => "–".into(),
            })
        }
    });
    price.connect_value_notify({
        let update = update_meals.clone();
        move |p| {
            let mut cfg = auth::config();
            cfg.mensa_meal_price = Some(p.value());
            if let Err(e) = auth::save_config(&cfg) {
                eprintln!("saving config: {e}");
            }
            update();
        }
    });

    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Refresh credit").css_classes(["flat"]).valign(gtk::Align::Center).build();
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.append(&button("Reserved area", |_| open_reserved_area()));
    header.append(&refresh);
    credit.set_header_suffix(Some(&header));
    let load_credit: Rc<dyn Fn()> = Rc::new({
        let (credit, balance, balance_label, price, update, refresh) = (credit.clone(), balance.clone(), balance_label.clone(), price.clone(), update_meals.clone(), refresh.clone());
        move || {
            let (credit, balance, balance_label, price, update, refresh) = (credit.clone(), balance.clone(), balance_label.clone(), price.clone(), update.clone(), refresh.clone());
            refresh.set_sensitive(false);
            balance_label.set_label("…");
            credit.set_description(Some("Signing in to the mensa…"));
            glib::spawn_future_local(async move {
                match account_text().await {
                    Ok(text) => {
                        let (b, p) = mensa::parse_account(&text);
                        balance.set(b);
                        balance_label.set_label(&b.map_or("?".into(), |b| format!("€ {b:.2}")));
                        // Use the site's meal price until the user sets one.
                        if let (Some(p), true) = (p, auth::config().mensa_meal_price.is_none()) {
                            price.set_value(p);
                        }
                        credit.set_description(if b.is_some() { None } else { Some("Could not find the credit on the reserved-area page. Use “Reserved area” to check it there.") });
                    }
                    Err(e) => {
                        balance_label.set_label("?");
                        credit.set_description(Some(&format!("Could not read the credit: {e}")));
                    }
                }
                update();
                refresh.set_sensitive(true);
            });
        }
    });
    refresh.connect_clicked({
        let load = load_credit.clone();
        move |_| load()
    });
    page.add(&credit);
    load_credit();

    // Menu.
    let location = adw::ComboRow::builder().title("Mensa").model(&gtk::StringList::new(&mensa::LOCATIONS.map(|l| l.1))).build();
    let current = cfg.mensa_location.clone().unwrap_or(mensa::LOCATIONS[0].0.into());
    location.set_selected(mensa::LOCATIONS.iter().position(|l| l.0 == current).unwrap_or(0) as u32);
    let settings = group("Menu");
    settings.set_header_suffix(Some(&link_button("Website", format!("{BASE}/menu"))));
    settings.add(&location);
    page.add(&settings);

    let days: Rc<RefCell<Vec<adw::PreferencesGroup>>> = Rc::default();
    let load_menu: Rc<dyn Fn()> = Rc::new({
        let (page, location, days) = (page.clone(), location.clone(), days.clone());
        move || {
            for g in days.borrow_mut().drain(..) {
                page.remove(&g);
            }
            let id = mensa::LOCATIONS[location.selected() as usize % mensa::LOCATIONS.len()].0.to_owned();
            let g = group("");
            page.add(&g);
            days.borrow_mut().push(g.clone());
            let (page, days) = (page.clone(), days.clone());
            fill(&g, move || mensa::menus(&id, 7), move |g, menus| {
                if menus.is_empty() {
                    g.add(&row("No menu published for the coming days", ""));
                }
                for (i, d) in menus.iter().enumerate() {
                    let dg = group(&day_title(d));
                    if !d.lunch.is_empty() {
                        dg.add(&meal_row("Lunch", &d.lunch, i == 0));
                    }
                    if !d.dinner.is_empty() {
                        dg.add(&meal_row("Dinner", &d.dinner, false));
                    }
                    page.add(&dg);
                    days.borrow_mut().push(dg);
                }
            });
        }
    });
    location.connect_selected_notify({
        let load = load_menu.clone();
        move |l| {
            let mut cfg = auth::config();
            cfg.mensa_location = Some(mensa::LOCATIONS[l.selected() as usize % mensa::LOCATIONS.len()].0.into());
            if let Err(e) = auth::save_config(&cfg) {
                eprintln!("saving config: {e}");
            }
            load();
        }
    });
    load_menu();
    page
}
