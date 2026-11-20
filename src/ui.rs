//! Shared widgets and helpers.
use adw::prelude::*;
use gtk::{gio, glib};
use webkit6::prelude::*;

use crate::api::{self, Dl, Item};
use crate::auth::Res;

pub fn esc(x: &str) -> String {
    glib::markup_escape_text(x).to_string()
}

pub fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder().title(esc(title)).subtitle(esc(subtitle)).build()
}

pub fn nav_row(title: &str, subtitle: &str, icon: &str) -> adw::ActionRow {
    let r = row(title, subtitle);
    r.add_prefix(&gtk::Image::from_icon_name(icon));
    r.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    r.set_activatable(true);
    r
}

pub fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(esc(title)).build()
}

pub fn button(label: &str, f: impl Fn(&gtk::Button) + 'static) -> gtk::Button {
    let b = gtk::Button::builder().label(label).css_classes(["flat"]).valign(gtk::Align::Center).build();
    b.connect_clicked(f);
    b
}

pub fn link_button(label: &str, url: String) -> gtk::Button {
    button(label, move |b| open_uri(b, &url))
}

pub fn window_of(w: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    w.root().and_downcast::<gtk::Window>()
}

pub fn open_uri(w: &impl IsA<gtk::Widget>, url: &str) {
    gtk::UriLauncher::new(url).launch(window_of(w).as_ref(), None::<&gio::Cancellable>, |r| {
        if let Err(e) = r {
            eprintln!("open: {e}");
        }
    });
}

/// Runs a blocking job off the main thread and hands the result back on it.
pub fn spawn<T: Send + 'static>(job: impl FnOnce() -> Res<T> + Send + 'static, done: impl FnOnce(Res<T>) + 'static) {
    glib::spawn_future_local(async move {
        done(gio::spawn_blocking(job).await.unwrap_or_else(|_| Err("background job panicked".into())));
    });
}

/// Fills `g` with the result of a blocking fetch, showing progress/errors in place.
pub fn fill<T: Send + 'static>(g: &adw::PreferencesGroup, job: impl FnOnce() -> Res<T> + Send + 'static, render: impl FnOnce(&adw::PreferencesGroup, T) + 'static) {
    let placeholder = row("Loading…", "");
    g.add(&placeholder);
    let g = g.clone();
    spawn(job, move |r| {
        g.remove(&placeholder);
        match r {
            Ok(v) => render(&g, v),
            Err(e) => g.add(&row("Could not load", &e)),
        }
    });
}

/// Opens the local copy of a file, downloading it first if missing or outdated.
pub fn download_and_open(w: &impl IsA<gtk::Widget>, dl: Dl, folder: String, on_done: impl FnOnce(Res<()>) + 'static) {
    let win = window_of(w);
    spawn(
        move || api::fetch_file(&folder, &dl),
        move |r| match r {
            Ok(path) => {
                gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(win.as_ref(), None::<&gio::Cancellable>, |_| {});
                on_done(Ok(()))
            }
            Err(e) => on_done(Err(e)),
        },
    );
}

fn file_icon(cached: bool) -> (&'static str, &'static str) {
    if cached { ("document-open-symbolic", "Saved locally") } else { ("folder-download-symbolic", "Download") }
}

pub fn item_row(it: &Item, folder: &str) -> adw::ActionRow {
    let r = row(&it.title, &it.subtitle);
    r.set_activatable(true);
    let (icon_name, tip) = match &it.download {
        Some(dl) => file_icon(api::is_cached(folder, dl)),
        None => ("adw-external-link-symbolic", "Open in browser"),
    };
    let icon = gtk::Image::builder().icon_name(icon_name).tooltip_text(tip).build();
    r.add_suffix(&icon);
    let (it, folder) = (it.clone(), folder.to_owned());
    r.connect_activated(move |r| match it.download.clone() {
        Some(dl) => {
            let (r2, sub, icon) = (r.clone(), it.subtitle.clone(), icon.clone());
            r.set_subtitle("Opening…");
            download_and_open(r, dl, folder.clone(), move |res| {
                if res.is_ok() {
                    let (name, tip) = file_icon(true);
                    icon.set_icon_name(Some(name));
                    icon.set_tooltip_text(Some(tip));
                }
                r2.set_subtitle(&esc(&match res {
                    Ok(()) => sub,
                    Err(e) => format!("Download failed: {e}"),
                }))
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

/// A page for an in-tab NavigationView, with its own slim header (back, title, actions).
pub fn subpage(title: &str, child: &impl IsA<gtk::Widget>, actions: &[gtk::Widget]) -> adw::NavigationPage {
    let bar = gtk::Box::builder().spacing(6).margin_start(6).margin_end(6).margin_top(6).margin_bottom(6).build();
    let back = gtk::Button::builder().icon_name("go-previous-symbolic").action_name("navigation.pop").css_classes(["flat"]).tooltip_text("Back").build();
    bar.append(&back);
    bar.append(&gtk::Label::builder().label(title).css_classes(["heading"]).ellipsize(gtk::pango::EllipsizeMode::End).hexpand(true).xalign(0.0).build());
    for a in actions {
        bar.append(a);
    }
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&bar);
    vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    child.set_vexpand(true);
    vbox.append(child);
    let page = adw::NavigationPage::builder().title(title).child(&vbox).build();
    // The root page cannot go back.
    page.connect_shown(move |p| back.set_visible(p.ancestor(adw::NavigationView::static_type()).and_downcast::<adw::NavigationView>().is_some_and(|v| v.previous_page(p).is_some())));
    page
}

/// Asks for a message (and optionally a subject). Returns None when cancelled.
pub async fn ask_text(parent: &impl IsA<gtk::Widget>, heading: &str, subject: Option<&str>, initial: &str) -> Option<(String, String)> {
    let dialog = adw::AlertDialog::new(Some(heading), None);
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", "Send")]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let entry = gtk::Entry::builder().placeholder_text("Subject").text(subject.unwrap_or_default()).build();
    if subject.is_some() {
        vbox.append(&entry);
    }
    let text = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).top_margin(6).bottom_margin(6).left_margin(6).right_margin(6).build();
    text.buffer().set_text(initial);
    vbox.append(&gtk::ScrolledWindow::builder().child(&text).min_content_height(180).min_content_width(420).css_classes(["card"]).build());
    dialog.set_extra_child(Some(&vbox));
    if dialog.choose_future(Some(parent)).await != "ok" {
        return None;
    }
    let b = text.buffer();
    Some((entry.text().into(), b.text(&b.start_iter(), &b.end_iter(), false).into()))
}

pub async fn confirm(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str, action: &str) -> bool {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", action)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.choose_future(Some(parent)).await == "ok"
}

/// Asks for a single line (e.g. an enrolment key). Returns None when cancelled.
pub async fn ask_line(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str, placeholder: &str) -> Option<String> {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", "OK")]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    let entry = gtk::PasswordEntry::builder().placeholder_text(placeholder).show_peek_icon(true).activates_default(true).build();
    dialog.set_extra_child(Some(&entry));
    dialog.set_default_response(Some("ok"));
    (dialog.choose_future(Some(parent)).await == "ok").then(|| entry.text().into())
}

pub fn alert(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}

/// Plain text as HTML paragraphs.
pub fn text_to_html(t: &str) -> String {
    t.split("\n\n").map(|p| format!("<p>{}</p>", esc(p).replace('\n', "<br>"))).collect()
}

const CSS: &str = "
:root { color-scheme: light dark; }
body { font: 15px/1.5 system-ui, sans-serif; margin: 0 auto; padding: 16px 24px 48px; max-width: 860px; }
img, video, iframe { max-width: 100%; height: auto; }
article { border: 1px solid #8884; border-radius: 12px; padding: 4px 16px; margin: 12px 0; }
article.reply { margin-left: 32px; }
.meta { opacity: .7; font-size: 13px; }
table { border-collapse: collapse; width: 100%; }
td, th { border-bottom: 1px solid #8884; padding: 6px 8px; text-align: left; vertical-align: top; }
.tag { display: inline-block; padding: 1px 8px; border-radius: 99px; background: #8883; font-size: 13px; }
a { color: #3584e4; }
";

/// Renders an HTML fragment. File links download to ~/Downloads/unibz/<folder>; other links open in the browser.
pub fn html_view(body: &str, folder: &str) -> webkit6::WebView {
    let view = crate::auth::web_view();
    let folder = folder.to_owned();
    view.connect_decide_policy(move |v, decision, _| {
        let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else { return false };
        let Some(action) = nav.navigation_action() else { return false };
        if action.navigation_type() != webkit6::NavigationType::LinkClicked {
            return false;
        }
        let Some(uri) = action.request().and_then(|r| r.uri()) else { return false };
        decision.ignore();
        if uri.contains("pluginfile.php") {
            let name = url::Url::parse(&uri)
                .ok()
                .and_then(|u| u.path_segments().and_then(|mut p| p.next_back().map(str::to_owned)))
                .map(|n| url::form_urlencoded::parse(format!("x={n}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or(n))
                .unwrap_or("file".into());
            // Attachment URLs change when the file does, so the URL (minus token) is the version.
            let version = uri.split("token=").next().unwrap_or(&uri).to_owned();
            let dl = Dl { url: uri.to_string(), rel: format!("Attachments/{name}"), version };
            download_and_open(v, dl, folder.clone(), |r| {
                if let Err(e) = r {
                    eprintln!("download: {e}");
                }
            });
        } else {
            open_uri(v, &uri);
        }
        true
    });
    set_html(&view, body);
    view
}

pub fn set_html(view: &webkit6::WebView, body: &str) {
    view.load_html(&format!("<!doctype html><meta charset=utf-8><style>{CSS}</style><body>{body}</body>"), Some(crate::auth::OLE));
}

/// HTML to readable plain text (for row subtitles and section summaries).
pub fn strip_html(h: &str) -> String {
    let mut out = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    for c in h.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let t = tag.trim_start_matches('/').split([' ', '/']).next().unwrap_or("").to_lowercase();
                if matches!(t.as_str(), "br" | "p" | "div" | "li" | "h1" | "h2" | "h3" | "h4" | "tr") {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let out = out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

pub fn fmt_ts(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts).ok().and_then(|d| d.format("%a %d %b %Y, %H:%M").ok()).map_or(String::new(), |s| s.to_string())
}

/// Local now as "YYYY-MM-DDTHH:MM:SS" (mySNS timetable times are local, without zone).
pub fn now_local() -> String {
    glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%dT%H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default()
}

/// Formats a zone-less local ISO time from mySNS.
pub fn fmt_local(iso: &str, fmt: &str) -> String {
    glib::DateTime::from_iso8601(iso, Some(&glib::TimeZone::local())).ok().and_then(|d| d.format(fmt).ok()).map_or(iso.to_owned(), |s| s.to_string())
}

pub fn fmt_iso(iso: &str) -> String {
    glib::DateTime::from_iso8601(iso, Some(&glib::TimeZone::utc()))
        .ok()
        .and_then(|d| d.to_local().ok())
        .and_then(|d| d.format("%a %d %b %Y, %H:%M").ok())
        .map_or(iso.to_owned(), |s| s.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn strip_html_keeps_text() {
        assert_eq!(super::strip_html("<p>Hello&nbsp;<b>world</b></p><p>a &amp; b</p>"), "Hello world\na & b");
    }
}
