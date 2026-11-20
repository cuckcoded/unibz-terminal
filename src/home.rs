//! Home screen: profile, study statistics and what's coming up across all courses.
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::api::{Course, Data, arr, s};
use crate::moodle;
use crate::ui::*;

fn stat_card(value: &str, caption: &str, extra: Option<&gtk::Widget>) -> gtk::Box {
    stat_card_with_label(value, caption, extra).0
}

fn stat_card_with_label(value: &str, caption: &str, extra: Option<&gtk::Widget>) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["card"]).build();
    let inner = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).margin_top(14).margin_bottom(14).margin_start(16).margin_end(16).build();
    let value_label = gtk::Label::builder().label(value).css_classes(["title-2"]).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
    inner.append(&value_label);
    inner.append(&gtk::Label::builder().label(caption).css_classes(["caption", "dim-label"]).xalign(0.0).wrap(true).build());
    if let Some(w) = extra {
        inner.append(w);
    }
    b.append(&inner);
    (b, value_label)
}

fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().filter(|x| *x > 0.0)
}

/// Mean of passed marks weighted by credits (fallback when mySNS has no average).
fn weighted_average(courses: &[Course]) -> Option<f64> {
    let (sum, w) = courses.iter().filter_map(|c| Some((c.mark? as f64, c.credits))).fold((0.0, 0.0), |(s, w), (m, c)| (s + m * c, w + c));
    (w > 0.0).then(|| sum / w)
}

/// `open` selects a sidebar entry ("c:<course id>").
pub fn page(data: &Data, ole: Result<String, String>, open: Rc<dyn Fn(&str)>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let (p, cr) = (&data.profile, &data.credits);
    let now = now_local();

    // Header.
    let g = adw::PreferencesGroup::new();
    let name = format!("{} {}", s(&p["firstName"]), s(&p["lastName"]));
    let header = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_bottom(6).build();
    let hello = if name.trim().is_empty() { "Welcome".into() } else { format!("Hi, {}", s(&p["firstName"])) };
    header.append(&gtk::Label::builder().label(hello).css_classes(["title-1"]).xalign(0.0).build());
    let degree = s(&p["degree"]["descriptionEn"]);
    if !degree.is_empty() {
        let line = format!("{degree} · year {} · {}", s(&p["enrolDegreeYear"]), s(&p["degree"]["faculty"]["descriptionEn"]));
        header.append(&gtk::Label::builder().label(line).css_classes(["dim-label"]).xalign(0.0).wrap(true).build());
    }
    g.add(&header);
    page.add(&g);

    // Stat cards.
    let passed: Vec<&Course> = data.courses.iter().filter(|c| c.passed()).collect();
    let ects_done = num(&cr["pointsCurricular"]).unwrap_or_else(|| passed.iter().map(|c| c.credits).sum());
    let ects_total = num(&cr["minimalPointsNecessary"]).map(|m| m + num(&cr["finalExamCreditPoints"]).unwrap_or(0.0)).unwrap_or(180.0);
    let avg30 = num(&cr["averageMark30"]).or_else(|| weighted_average(&data.courses));
    let avg110 = num(&cr["averageMark"]);
    let week_end = glib::DateTime::now_local().and_then(|d| d.add_days(7)).ok().and_then(|d| d.format("%Y-%m-%dT%H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
    let mut week: Vec<(&Course, &crate::api::Lecture)> =
        data.courses.iter().flat_map(|c| c.lectures.iter().map(move |l| (c, l))).filter(|(_, l)| l.kind != "EXAM" && l.end >= now && l.start <= week_end).collect();
    week.sort_by(|a, b| a.1.start.cmp(&b.1.start));
    let mut exams: Vec<(&Course, (String, String))> = data.courses.iter().filter(|c| !c.passed()).filter_map(|c| Some((c, c.next_exam(&now)?))).collect();
    exams.sort_by(|a, b| a.1.0.cmp(&b.1.0));

    let flow = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(true).min_children_per_line(2).max_children_per_line(4).column_spacing(12).row_spacing(12).build();
    let bar = gtk::LevelBar::builder().min_value(0.0).max_value(ects_total.max(1.0)).value(ects_done.min(ects_total)).margin_top(6).build();
    flow.append(&stat_card(&format!("{ects_done:.0} / {ects_total:.0}"), &format!("ECTS ({:.0}%)", 100.0 * ects_done / ects_total.max(1.0)), Some(bar.upcast_ref())));
    flow.append(&stat_card(&avg30.map_or("–".into(), |a| format!("{a:.2}")), "Weighted average (out of 30)", None));
    flow.append(&stat_card(&avg110.map_or("–".into(), |a| format!("{a:.1}")), "Projected base grade (out of 110)", None));
    let cum = passed.iter().filter(|c| c.cum_laude).count();
    flow.append(&stat_card(
        &format!("{} / {}", passed.len(), data.courses.len()),
        &if cum > 0 { format!("Exams passed · {cum} cum laude") } else { "Exams passed".into() },
        None,
    ));
    flow.append(&stat_card(
        &exams.first().map_or("–".into(), |(_, (when, _))| fmt_local(when, "%a %d %b")),
        &exams.first().map_or("No exam scheduled".into(), |(c, _)| format!("Next exam: {}", c.name)),
        None,
    ));
    flow.append(&stat_card(&week.len().to_string(), "Lectures in the next 7 days", None));
    let (card, credit) = stat_card_with_label("…", "Mensa credit", None);
    card.set_tooltip_text(Some("Open the mensa"));
    card.set_cursor_from_name(Some("pointer"));
    let click = gtk::GestureClick::new();
    let open_mensa = open.clone();
    click.connect_released(move |_, _, _, _| open_mensa("m:mensa"));
    card.add_controller(click);
    glib::spawn_future_local(async move {
        match crate::mensa_ui::credit(false).await {
            Ok(v) => credit.set_label(&crate::mensa_ui::euros(v)),
            Err(e) => {
                credit.set_label("?");
                credit.set_tooltip_text(Some(&e));
            }
        }
    });
    flow.append(&card);
    let g = adw::PreferencesGroup::new();
    g.add(&flow);
    page.add(&g);

    let course_row = |title: &str, sub: &str, c: &Course| {
        let r = row(title, sub);
        r.set_activatable(true);
        r.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (open, key) = (open.clone(), format!("c:{}", c.id));
        r.connect_activated(move |_| open(&key));
        r
    };

    let g = group("Coming up this week");
    if week.is_empty() {
        g.add(&row("No lectures in the next 7 days", ""));
    }
    for (c, l) in week.iter().take(15) {
        g.add(&course_row(&format!("{}–{} · {}", fmt_local(&l.start, "%a %d %b, %H:%M"), l.end.get(11..16).unwrap_or(""), c.name), &format!("{} · {}", l.activity, l.room), c));
    }
    page.add(&g);

    let g = group("Upcoming exams");
    if exams.is_empty() {
        g.add(&row("No exam appointments in mySNS", ""));
    }
    for (c, (when, desc)) in &exams {
        g.add(&course_row(&format!("{} · {}", fmt_local(when, "%a %d %b %Y, %H:%M"), c.name), desc, c));
    }
    page.add(&g);

    let g = group("OLE deadlines");
    match ole {
        Ok(token) => {
            let from = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or_default();
            fill(
                &g,
                move || moodle::call(&token, "core_calendar_get_action_events_by_timesort", &[("timesortfrom", from.to_string()), ("limitnum", "25".into())]),
                |g, v| {
                    let evs = arr(&v["events"]);
                    if evs.is_empty() {
                        g.add(&row("Nothing due on OLE", ""));
                    }
                    for e in evs {
                        let r = row(&s(&e["name"]), &format!("{} · {}", fmt_ts(e["timesort"].as_i64().unwrap_or_default()), s(&e["course"]["fullname"])));
                        r.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
                        r.set_activatable(true);
                        let url = s(&e["url"]);
                        r.connect_activated(move |r| open_uri(r, &url));
                        g.add(&r);
                    }
                },
            );
        }
        Err(e) => g.add(&row("OLE unavailable", &e)),
    }
    page.add(&g);

    let mut recent: Vec<&&Course> = passed.iter().filter(|c| c.exam_date.is_some()).collect();
    recent.sort_by(|a, b| b.exam_date.cmp(&a.exam_date));
    let g = group("Recent results");
    for c in recent.iter().take(6) {
        let mark = c.mark.map_or(c.status.clone(), |m| if c.cum_laude { format!("{m} cum laude") } else { m.to_string() });
        g.add(&course_row(&format!("{mark} · {}", c.name), &format!("{} · {} ECTS", c.exam_date.clone().unwrap_or_default(), c.credits), c));
    }
    if !recent.is_empty() {
        page.add(&g);
    }

    let g = group("Profile");
    for (label, v) in [
        ("Name", name),
        ("Matriculation number", s(&p["registrationNumber"])),
        ("Email", s(&p["emailAddress"])),
        ("Enrolled since", s(&p["enrolmentYear"])),
        ("Status", s(&p["status"]["descriptionEn"])),
        ("Library number", s(&p["libraryNo"])),
    ] {
        if !v.trim().is_empty() {
            let r = row(label, "");
            r.add_suffix(&gtk::Label::builder().label(v.trim()).selectable(true).css_classes(["dim-label"]).build());
            g.add(&r);
        }
    }
    page.add(&g);

    if !data.warnings.is_empty() {
        let g = group("Problems while loading");
        for w in &data.warnings {
            let r = row(w, "");
            r.set_title_lines(0);
            g.add(&r);
        }
        page.add(&g);
    }
    page
}

#[cfg(test)]
mod tests {
    #[test]
    fn weighted_average_uses_credits() {
        let c = |mark, credits| crate::api::Course { mark: Some(mark), credits, ..Default::default() };
        assert_eq!(super::weighted_average(&[c(30, 6.0), c(18, 12.0)]), Some(22.0));
        assert_eq!(super::weighted_average(&[]), None);
    }
}
