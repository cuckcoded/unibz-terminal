//! Mensa (unibz.markas.info): public menu scraping and reserved-area text parsing.
use crate::auth::{Res, err};
use crate::ui::strip_html;

pub const BASE: &str = "https://unibz.markas.info";
/// "SNS Access UNIBZ" login button.
pub const SNS_LOGIN: &str = "https://unibz.markas.info/api/auth/v1/oauth/request/2?OutputFormat=Web";
pub const AREA: &str = "https://unibz.markas.info/area-riservata";
/// (menu id, name) as offered by the site.
pub const LOCATIONS: [(&str, &str); 3] = [("3783", "Bolzano"), ("3799", "Bressanone"), ("8211", "NOI Techpark")];

#[derive(Clone, Debug, PartialEq)]
pub struct Dish {
    pub category: String,
    pub name: String,
    pub allergens: String,
    pub url: String,
    pub image: String,
}

pub const LUNCH: u8 = 3;
pub const DINNER: u8 = 5;

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    Some(&s[i..i + s[i..].find(end)?])
}

/// Returns the day the page shows (dd/mm/yyyy) and its dishes.
pub fn parse_menu(html: &str) -> (Option<String>, Vec<Dish>) {
    let day = between(html, "class=\"squared_day selected", "</div>").and_then(|d| between(d, "data-day=\"", "\"")).map(str::to_owned);
    let mut cats = std::collections::HashMap::new();
    for part in html.split("data-filter=\".").skip(1) {
        if let (Some(id), Some(label)) = (part.split('"').next(), between(part, "class=\"cbp-filter-item\">", "<")) {
            if !id.contains(',') {
                cats.insert(id.to_owned(), strip_html(label));
            }
        }
    }
    let mut dishes = Vec::new();
    for item in html.split("class=\"pointer cbp-item").skip(1) {
        let classes = item.split('"').next().unwrap_or("");
        let cat = classes.split_whitespace().find(|c| c.starts_with("dt_")).unwrap_or("");
        let Some(name) = between(item, "projects-title\">", "<").map(strip_html).filter(|n| !n.is_empty()) else { continue };
        let desc = between(item, "projects-desc\">", "<!--").unwrap_or("");
        let allergens: Vec<String> = desc.split("alt=\"").skip(1).filter_map(|a| a.split('"').next()).map(|a| strip_html(&a.replace("&lt;", "<").replace("&gt;", ">"))).collect();
        dishes.push(Dish {
            category: cats.get(cat).cloned().unwrap_or_default(),
            name,
            allergens: allergens.join(", "),
            url: between(item, "href=\"", "\"").filter(|u| u.contains("/piatto/")).unwrap_or_default().to_owned(),
            // The first image of an item is the dish photo; allergen icons come later.
            image: between(item, "src=\"", "\"").filter(|u| u.contains("/images/piatti/")).unwrap_or_default().to_owned(),
        });
    }
    (day, dishes)
}

static LANG_COOKIE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Session cookie with the language set to English (the language lives in the server session).
fn english_session() -> String {
    let mut c = LANG_COOKIE.lock().unwrap();
    c.get_or_insert_with(|| {
        // The cookie comes with the redirect, so don't follow it.
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        match agent.get(&format!("{BASE}/change_language/en")).call() {
            Ok(r) => r.all("set-cookie").into_iter().filter_map(|c| c.split(';').next()).filter(|c| c.starts_with("markas_ci_session=") && c.len() > 20).last().unwrap_or_default().to_owned(),
            Err(_) => String::new(),
        }
    })
    .clone()
}

/// One meal (LUNCH or DINNER) of one day; empty when nothing is published for that day.
pub fn day(location: &str, meal: u8, date: &gtk::glib::DateTime) -> Res<Vec<Dish>> {
    let want = date.format("%d/%m/%Y").map_err(err)?.to_string();
    let html = ureq::get(&format!("{BASE}/menu/{}/0/{location}/{meal}", date.to_unix())).set("Cookie", &english_session()).call().map_err(err)?.into_string().map_err(err)?;
    let (shown, dishes) = parse_menu(&html);
    // The site falls back to another day when there is no menu for this one.
    Ok(if shown.as_deref() == Some(want.as_str()) { dishes } else { Vec::new() })
}

/// Local copy of a dish photo (cached; photos don't change).
pub fn image(url: &str) -> Res<std::path::PathBuf> {
    let name = url.rsplit('/').next().unwrap_or("dish.jpg").replace(['\\', '\0'], "_");
    let dir = gtk::glib::user_cache_dir().join("unibz-terminal").join("mensa");
    let path = dir.join(if name.is_empty() || name.starts_with('.') { "dish.jpg" } else { &name });
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(err)?;
    let part = path.with_extension("part");
    std::io::copy(&mut ureq::get(url).call().map_err(err)?.into_reader(), &mut std::fs::File::create(&part).map_err(err)?).map_err(err)?;
    std::fs::rename(&part, &path).map_err(err)?;
    Ok(path)
}

/// First money amount in a line: "12,40", "€ 3.10", "-1.50 €".
fn amount(line: &str) -> Option<f64> {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let whole = &line[start..i];
            let neg = line[..start].trim_end_matches(['€', ' ']).ends_with('-');
            let euro_near = line[..start].trim_end().ends_with('€') || line[i..].trim_start().starts_with('€');
            if b.get(i).is_some_and(|c| *c == b',' || *c == b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) && b.get(i + 2).is_some_and(u8::is_ascii_digit) && !b.get(i + 3).is_some_and(u8::is_ascii_digit) {
                let v: f64 = format!("{whole}.{}", &line[i + 1..i + 3]).parse().ok()?;
                return Some(if neg { -v } else { v });
            }
            if euro_near {
                let v: f64 = whole.parse().ok()?;
                return Some(if neg { -v } else { v });
            }
        } else {
            i += 1;
        }
    }
    None
}

fn find_amount(lines: &[&str], keys: &[&str]) -> Option<f64> {
    for (i, l) in lines.iter().enumerate() {
        let low = l.to_lowercase();
        if keys.iter().any(|k| low.contains(k)) {
            // The value is either on the label's line or right below it.
            if let Some(a) = amount(l).or_else(|| lines.get(i + 1).and_then(|n| amount(n))) {
                return Some(a);
            }
        }
    }
    None
}

/// Credit balance from the reserved area's visible text, in any of the site's languages.
pub fn parse_balance(text: &str) -> Option<f64> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    find_amount(&lines, &["balance", "credit", "saldo", "credito", "guthaben", "borsellino", "wallet"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(amount("Saldo: € 12,40"), Some(12.40));
        assert_eq!(amount("Balance -1.50 €"), Some(-1.50));
        assert_eq!(amount("Credit 7 €"), Some(7.0));
        assert_eq!(amount("Week 47 of 2026"), None);
    }

    #[test]
    fn account_text() {
        let t = "Hello\nYour credit\n€ 23,10\nTariff band 2: 3,50 €\nTop up";
        assert_eq!(parse_balance(t), Some(23.10));
        // As on the real reserved-area page.
        assert_eq!(parse_balance("Digital Wallet RistoCloud\nSaldo attuale: € 7,71\nRicarica"), Some(7.71));
    }

    #[test]
    fn menu_html() {
        let h = r#"<div class="squared_day selected " data-day="20/11/2026"><a></a></div>
<div data-filter=".dt_1, .dt_2" class="x">All</div><div data-filter=".dt_1" class="cbp-filter-item">First courses<div></div></div>
<div data-id="1" data-price="0" class="pointer cbp-item dt_1 allergene_37"><a href="https://unibz.markas.info/piatto/1-x/3783/3/1"><img width="100%" src="https://unibz.markas.info/images/piatti/1-x.jpg"></a>
<div class="cbp-l-grid-masonry-projects-title">Pasta &amp; pesto</div> <div class="cbp-l-grid-masonry-projects-desc">&nbsp; <img alt="Cereali: <strong>Grano</strong>"></div> <!-- x --></div>"#;
        let (day, dishes) = parse_menu(h);
        assert_eq!(day.as_deref(), Some("20/11/2026"));
        assert_eq!(dishes, vec![Dish { category: "First courses".into(), name: "Pasta & pesto".into(), allergens: "Cereali: Grano".into(), url: "https://unibz.markas.info/piatto/1-x/3783/3/1".into(), image: "https://unibz.markas.info/images/piatti/1-x.jpg".into() }]);
    }
}

#[cfg(test)]
mod live {
    /// Hits the public menu pages: `cargo test live_menu -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_menu() {
        let today = gtk::glib::DateTime::now_local().unwrap();
        let dishes = super::day("3783", super::LUNCH, &today).unwrap();
        println!("{} dishes, e.g. {:?}", dishes.len(), dishes.first());
        if let Some(d) = dishes.iter().find(|d| !d.image.is_empty()) {
            println!("image at {:?}", super::image(&d.image).unwrap());
        }
    }
}
