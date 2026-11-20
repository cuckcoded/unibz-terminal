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
/// Downloads (or reuses the cached copy) and shows the file in the course page's preview pane,
/// or opens it in another app where there is no preview pane.
pub fn download_and_open(w: &impl IsA<gtk::Widget>, dl: Dl, folder: String, on_done: impl FnOnce(Res<()>) + 'static) {
    let w = w.clone().upcast::<gtk::Widget>();
    spawn(
        move || api::fetch_file(&folder, &dl),
        move |r| match r {
            Ok(path) => {
                if !show_preview(&w, &path) {
                    launch_file(&w, &path);
                }
                on_done(Ok(()))
            }
            Err(e) => on_done(Err(e)),
        },
    );
}

fn launch_file(w: &gtk::Widget, path: &std::path::Path) {
    gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(window_of(w).as_ref(), None::<&gio::Cancellable>, |_| {});
}

const PREVIEW_HOST: &str = "preview-host";

/// Wraps a course page so files opened inside it are previewed on the right.
pub fn with_preview(content: &impl IsA<gtk::Widget>) -> adw::OverlaySplitView {
    adw::OverlaySplitView::builder()
        .content(content)
        .sidebar_position(gtk::PackType::End)
        .show_sidebar(false)
        .sidebar_width_fraction(0.5)
        .min_sidebar_width(360.0)
        .max_sidebar_width(1400.0)
        .name(PREVIEW_HOST)
        .build()
}

/// Shows `path` in the preview pane of the course page `from` is in; false if it isn't in one.
fn show_preview(from: &gtk::Widget, path: &std::path::Path) -> bool {
    let Some(split) = from.ancestor(adw::OverlaySplitView::static_type()).and_downcast::<adw::OverlaySplitView>().filter(|s| s.widget_name() == PREVIEW_HOST) else {
        return false;
    };
    let bar = gtk::Box::builder().spacing(6).margin_start(12).margin_end(6).margin_top(6).margin_bottom(6).build();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    bar.append(&gtk::Label::builder().label(&name).tooltip_text(&name).css_classes(["heading"]).ellipsize(gtk::pango::EllipsizeMode::Middle).hexpand(true).xalign(0.0).build());
    let open = gtk::Button::builder().icon_name("document-open-symbolic").tooltip_text("Open in another app").css_classes(["flat"]).build();
    let p = path.to_owned();
    open.connect_clicked(move |b| launch_file(b.upcast_ref(), &p));
    let close = gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text("Close preview").css_classes(["flat"]).build();
    let s2 = split.clone();
    close.connect_clicked(move |_| s2.set_show_sidebar(false));
    bar.append(&open);
    bar.append(&close);
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.append(&bar);
    pane.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let content = preview_content(path);
    content.set_vexpand(true);
    pane.append(&content);
    split.set_sidebar(Some(&pane));
    split.set_show_sidebar(true);
    true
}

/// Office formats are previewed as PDF through LibreOffice.
const OFFICE: [&str; 11] = ["doc", "docx", "ppt", "pptx", "pps", "ppsx", "xls", "xlsx", "odt", "odp", "ods"];

fn pdf_view(path: &std::path::Path) -> gtk::Widget {
    let v = webkit6::WebView::new();
    v.load_uri(&gio::File::for_path(path).uri());
    v.upcast()
}

fn no_preview(path: &std::path::Path, why: &str) -> gtk::Widget {
    let p = path.to_owned();
    let b = gtk::Button::builder().label("Open in another app").halign(gtk::Align::Center).css_classes(["pill", "suggested-action"]).build();
    b.connect_clicked(move |b| launch_file(b.upcast_ref(), &p));
    adw::StatusPage::builder().icon_name("text-x-generic-symbolic").title("No preview").description(esc(why)).child(&b).build().upcast()
}

fn preview_content(path: &std::path::Path) -> gtk::Widget {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ctype = gio::content_type_guess(Some(path), None).0;
    let mime = gio::content_type_get_mime_type(&ctype).map(|m| m.to_string()).unwrap_or_default();
    if mime.starts_with("image/") {
        return gtk::Picture::builder().file(&gio::File::for_path(path)).content_fit(gtk::ContentFit::Contain).can_shrink(true).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build().upcast();
    }
    if mime == "application/pdf" || mime == "text/html" {
        return pdf_view(path);
    }
    if mime.starts_with("video/") || mime.starts_with("audio/") {
        return gtk::Video::builder().file(&gio::File::for_path(path)).autoplay(false).build().upcast();
    }
    if OFFICE.contains(&ext.as_str()) {
        let holder = adw::Bin::builder().child(&adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build()).build();
        let (src, orig, h) = (path.to_owned(), path.to_owned(), holder.clone());
        spawn(move || office_to_pdf(&src), move |r| {
            h.set_child(Some(&match r {
                Ok(pdf) => pdf_view(&pdf),
                Err(e) => no_preview(&orig, &e),
            }))
        });
        return holder.upcast();
    }
    if gio::content_type_is_a(&ctype, "text/plain") || mime == "application/json" || mime.ends_with("+xml") || mime == "application/xml" {
        let bytes = std::fs::read(path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(2 << 20)]).to_string();
        let tv = gtk::TextView::builder().editable(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).left_margin(12).right_margin(12).top_margin(12).bottom_margin(12).build();
        tv.buffer().set_text(&text);
        return gtk::ScrolledWindow::builder().child(&tv).build().upcast();
    }
    no_preview(path, &format!("This app can't show {} files.", if ext.is_empty() { "these" } else { &ext }))
}

/// Converts an Office document to PDF (cached next to other previews); needs LibreOffice.
fn office_to_pdf(src: &std::path::Path) -> Res<std::path::PathBuf> {
    let key = glib::compute_checksum_for_string(glib::ChecksumType::Sha256, src.to_string_lossy().as_ref()).map(|k| k.to_string()).unwrap_or_default();
    let dir = glib::user_cache_dir().join("unibz-terminal").join("previews").join(key.get(..16).unwrap_or("x"));
    let pdf = dir.join(src.with_extension("pdf").file_name().unwrap_or_default());
    let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if pdf.is_file() && modified(&pdf) >= modified(src) {
        return Ok(pdf);
    }
    std::fs::create_dir_all(&dir).map_err(crate::auth::err)?;
    // A separate profile, so a LibreOffice window that is already open doesn't swallow the job.
    let profile = gio::File::for_path(glib::user_cache_dir().join("unibz-terminal").join("lo-profile")).uri();
    let out = std::process::Command::new("soffice")
        .arg(format!("-env:UserInstallation={profile}"))
        .args(["--headless", "--convert-to", "pdf", "--outdir"])
        .arg(&dir)
        .arg(src)
        .output()
        .map_err(|_| "Install LibreOffice to preview Office documents.".to_string())?;
    if pdf.is_file() { Ok(pdf) } else { Err(format!("LibreOffice could not convert it: {}", String::from_utf8_lossy(&out.stderr).trim())) }
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

mod masonry_imp {
    use gtk::{glib, prelude::*, subclass::prelude::*};

    pub const COL_MIN: i32 = 380;
    pub const GAP: i32 = 24;
    pub const MAX_COLS: i32 = 3;

    #[derive(Default)]
    pub struct Masonry;

    #[glib::object_subclass]
    impl ObjectSubclass for Masonry {
        const NAME: &'static str = "UnibzMasonry";
        type Type = super::Masonry;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for Masonry {}

    fn children(w: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut out = Vec::new();
        let mut c = w.first_child();
        while let Some(x) = c {
            c = x.next_sibling();
            if x.should_layout() {
                out.push(x);
            }
        }
        out
    }

    /// Narrowest a column may be: the default, or wider if a child needs it.
    fn col_min(w: &gtk::Widget) -> i32 {
        children(w).iter().map(|c| c.measure(gtk::Orientation::Horizontal, -1).0).max().unwrap_or(0).max(COL_MIN)
    }

    /// Puts each child into the currently shortest column. Returns (child, x, y, w, h) and the total height.
    pub fn place(w: &gtk::Widget, width: i32) -> (Vec<(gtk::Widget, i32, i32, i32, i32)>, i32) {
        let min = col_min(w);
        let cols = ((width + GAP) / (min + GAP)).clamp(1, MAX_COLS);
        let colw = ((width - GAP * (cols - 1)) / cols).max(min.min(width));
        let mut heights = vec![0; cols as usize];
        let mut out = Vec::new();
        for c in children(w) {
            let col = (0..cols as usize).min_by_key(|&i| heights[i]).unwrap_or(0);
            let h = c.measure(gtk::Orientation::Vertical, colw).1;
            out.push((c, col as i32 * (colw + GAP), heights[col], colw, h));
            if h > 0 {
                heights[col] += h + GAP;
            }
        }
        (out, (heights.into_iter().max().unwrap_or(0) - GAP).max(0))
    }

    impl LayoutManagerImpl for Masonry {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, w: &gtk::Widget, o: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let min = col_min(w);
            if o == gtk::Orientation::Horizontal {
                (min, MAX_COLS * min + (MAX_COLS - 1) * GAP, -1, -1)
            } else {
                let h = place(w, if for_size < 0 { min } else { for_size }).1;
                (h, h, -1, -1)
            }
        }

        fn allocate(&self, w: &gtk::Widget, width: i32, _height: i32, _baseline: i32) {
            for (c, x, y, cw, ch) in place(w, width).0 {
                c.size_allocate(&gtk::Allocation::new(x, y, cw, ch), -1);
            }
        }
    }
}

glib::wrapper! {
    /// Masonry layout: children flow into 1–3 columns depending on the width.
    pub struct Masonry(ObjectSubclass<masonry_imp::Masonry>) @extends gtk::LayoutManager;
}

/// Drop-in for adw::PreferencesPage that uses wide windows: groups go into as many columns as fit.
#[derive(Clone)]
pub struct WidePage {
    pub root: gtk::ScrolledWindow,
    top: gtk::Box,
    cols: gtk::Box,
}

impl WidePage {
    pub fn new() -> Self {
        let top = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
        let cols = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cols.set_layout_manager(Some(glib::Object::new::<Masonry>()));
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(24).margin_top(24).margin_bottom(24).margin_start(18).margin_end(18).build();
        content.append(&top);
        content.append(&cols);
        let clamp = adw::Clamp::builder().maximum_size(1400).tightening_threshold(1000).child(&content).build();
        let root = gtk::ScrolledWindow::builder().child(&clamp).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        Self { root, top, cols }
    }

    /// Full width, above the columns.
    pub fn add_top(&self, w: &impl IsA<gtk::Widget>) {
        self.top.append(w);
    }

    pub fn add(&self, w: &impl IsA<gtk::Widget>) {
        self.cols.append(w);
    }

    pub fn remove(&self, w: &impl IsA<gtk::Widget>) {
        if w.parent().as_ref() == Some(self.cols.upcast_ref()) {
            self.cols.remove(w);
        } else if w.parent().as_ref() == Some(self.top.upcast_ref()) {
            self.top.remove(w);
        }
    }
}

/// App-wide stylesheet pieces; call once at startup.
pub fn load_css(parts: &[&str]) {
    let p = gtk::CssProvider::new();
    p.load_from_string(&parts.concat());
    if let Some(d) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&d, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

#[cfg(test)]
mod masonry_tests {
    use adw::prelude::*;

    #[test]
    fn columns_by_width() {
        gtk::init().unwrap();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for h in [300, 100, 100, 100] {
            b.append(&gtk::Box::builder().height_request(h).build());
        }
        let w: gtk::Widget = b.into();
        // Narrow: one column, stacked.
        let (one, total) = super::masonry_imp::place(&w, 400);
        assert!(one.iter().all(|c| c.1 == 0));
        assert_eq!(total, 300 + 100 * 3 + 24 * 3);
        // Wide: the tall one gets its own column, the rest fill the shortest columns.
        let (three, total) = super::masonry_imp::place(&w, 1300);
        assert_eq!(three.iter().map(|c| c.1 > 0).filter(|x| *x).count(), 3);
        assert_eq!(total, 300);
    }
}
