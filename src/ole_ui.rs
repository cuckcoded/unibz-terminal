//! OLE tab: a native client for an OLE (Moodle) course. Activity types without a native
//! view open the real OLE page in an embedded, auto-logged-in browser.
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use webkit6::prelude::WebViewExt;
use serde_json::Value;

use crate::api::{Item, arr, s};
use crate::auth::{OLE, Res, err};
use crate::moodle::{self, call, file_url, fix_html};
use crate::ui::*;

pub struct Ole {
    pub token: String,
    pub private: Option<String>,
    pub user: Option<i64>,
    pub course: i64,
    pub name: String,
    /// Download folder name.
    pub folder: String,
}

impl Ole {
    fn call(self: &Rc<Self>, func: &'static str, args: Vec<(&'static str, String)>, done: impl FnOnce(Res<Value>) + 'static) {
        let token = self.token.clone();
        spawn(move || call(&token, func, &args), done);
    }
    fn html(&self, h: &str) -> String {
        fix_html(h, &self.token)
    }
}

pub fn tab(o: Rc<Ole>) -> adw::NavigationView {
    let nav = adw::NavigationView::new();
    let page = adw::PreferencesPage::new();
    let g = group("Course");
    g.set_header_suffix(Some(&link_button("Open in browser", format!("{OLE}/course/view.php?id={}", o.course))));
    for (title, icon, which) in [("Grades", "starred-symbolic", 0), ("Participants", "system-users-symbolic", 1), ("Upcoming deadlines", "alarm-symbolic", 2), ("Course page on OLE", "web-browser-symbolic", 3)] {
        let r = nav_row(title, "", icon);
        let (o, nav2) = (o.clone(), nav.clone());
        r.connect_activated(move |_| {
            let p = match which {
                0 => grades_page(&o),
                1 => participants_page(&o, &nav2),
                2 => upcoming_page(&o, &nav2),
                _ => web_page(&o, &format!("{OLE}/course/view.php?id={}", o.course), &o.name),
            };
            nav2.push(&p);
        });
        g.add(&r);
    }
    page.add(&g);
    let loading = group("");
    page.add(&loading);
    let (o2, nav2, page2) = (o.clone(), nav.clone(), page.clone());
    fill(
        &loading,
        {
            let token = o.token.clone();
            let id = o.course;
            move || call(&token, "core_course_get_contents", vec![("courseid", id.to_string())].as_slice())
        },
        move |loading, v| {
            page2.remove(loading);
            for sec in arr(&v) {
                if sec["uservisible"] == false && sec["visible"] == 0 {
                    continue;
                }
                let g = group(&Some(s(&sec["name"])).filter(|n| !n.is_empty()).unwrap_or("General".into()));
                let summary = strip_html(&s(&sec["summary"]));
                if !summary.is_empty() {
                    g.set_description(Some(&summary));
                }
                for m in arr(&sec["modules"]) {
                    g.add(&module_row(&o2, &nav2, m));
                }
                page2.add(&g);
            }
        },
    );
    nav.push(&subpage(&o.name, &page, &[]));
    nav
}

fn icon_for(modname: &str) -> &'static str {
    match modname {
        "resource" => "text-x-generic-symbolic",
        "folder" => "folder-symbolic",
        "url" => "adw-external-link-symbolic",
        "page" | "book" => "accessories-text-editor-symbolic",
        "forum" => "mail-unread-symbolic",
        "assign" => "document-send-symbolic",
        "quiz" => "dialog-question-symbolic",
        _ => "applications-other-symbolic",
    }
}

fn module_row(o: &Rc<Ole>, nav: &adw::NavigationView, m: &Value) -> gtk::Widget {
    let (name, kind) = (s(&m["name"]), s(&m["modname"]));
    if kind == "label" {
        let r = adw::ActionRow::builder().title(esc(&strip_html(&s(&m["description"])))).title_lines(0).build();
        r.add_css_class("property");
        return r.upcast();
    }
    let mut sub = vec![kind.clone()];
    for d in arr(&m["dates"]) {
        sub.push(format!("{} {}", s(&d["label"]), fmt_ts(d["timestamp"].as_i64().unwrap_or_default())));
    }
    let r = row(&name, &sub.join(" · "));
    r.add_prefix(&gtk::Image::from_icon_name(icon_for(&kind)));
    if m["uservisible"] == false {
        r.set_subtitle(&esc(&strip_html(&s(&m["availabilityinfo"]))));
        r.set_sensitive(false);
        return r.upcast();
    }
    let cd = &m["completiondata"];
    if cd.is_object() {
        let done = cd["state"].as_i64().unwrap_or_default() >= 1;
        if cd["isautomatic"].as_bool().unwrap_or(m["completion"] == 2) {
            if done {
                r.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
        } else {
            let check = gtk::CheckButton::builder().active(done).valign(gtk::Align::Center).tooltip_text("Mark as done").build();
            let (o, cmid) = (o.clone(), s(&m["id"]));
            check.connect_toggled(move |c| {
                let c = c.clone();
                o.call("core_completion_update_activity_completion_status_manually", vec![("cmid", cmid.clone()), ("completed", (c.is_active() as i32).to_string())], move |r| {
                    if let Err(e) = r {
                        c.set_tooltip_text(Some(&format!("Could not save: {e}")));
                    }
                });
            });
            r.add_suffix(&check);
        }
    }
    r.set_activatable(true);
    let (o, nav, m) = (o.clone(), nav.clone(), m.clone());
    r.connect_activated(move |r| open_module(&o, &nav, &m, r));
    r.upcast()
}

fn files_of(o: &Ole, m: &Value) -> Vec<Item> {
    arr(&m["contents"])
        .iter()
        .filter(|f| f["type"] == "file")
        .map(|f| {
            let path = s(&f["filepath"]);
            Item {
                title: s(&f["filename"]),
                subtitle: if path == "/" { String::new() } else { path.trim_matches('/').into() },
                download: Some((file_url(&s(&f["fileurl"]), &o.token), s(&f["filename"]))),
                web: None,
            }
        })
        .collect()
}

fn open_module(o: &Rc<Ole>, nav: &adw::NavigationView, m: &Value, r: &adw::ActionRow) {
    let (name, url) = (s(&m["name"]), s(&m["url"]));
    let files = files_of(o, m);
    match s(&m["modname"]).as_str() {
        "url" => open_uri(r, &arr(&m["contents"]).first().map(|c| s(&c["fileurl"])).filter(|u| !u.is_empty()).unwrap_or(url)),
        "resource" if files.len() == 1 => {
            let (r2, it) = (r.clone(), files[0].clone());
            let (dl, file) = it.download.unwrap();
            r.set_subtitle("Downloading…");
            download_and_open(r, dl, o.folder.clone(), file, move |res| r2.set_subtitle(&esc(&res.err().unwrap_or("resource".into()))));
        }
        "resource" | "folder" => {
            let page = adw::PreferencesPage::new();
            let g = group("");
            for it in &files {
                g.add(&item_row(it, &o.folder));
            }
            page.add(&g);
            nav.push(&subpage(&name, &page, &[link_button("Open in browser", url).upcast()]));
        }
        "page" => nav.push(&page_page(o, m)),
        "forum" => nav.push(&forum_page(o, nav, m["instance"].as_i64().unwrap_or_default(), &name)),
        "assign" => nav.push(&assign_page(o, m["id"].as_i64().unwrap_or_default(), &name)),
        _ => nav.push(&web_page(o, &url, &name)),
    }
}

/// The real OLE page, logged in via the mobile autologin key when possible.
fn web_page(o: &Rc<Ole>, url: &str, title: &str) -> adw::NavigationPage {
    let view = crate::auth::web_view();
    let (token, private, user, u, v) = (o.token.clone(), o.private.clone(), o.user, url.to_owned(), view.clone());
    spawn(move || Ok(moodle::autologin(&token, private.as_deref(), user, &u)), move |r: Res<String>| v.load_uri(&r.unwrap_or_default()));
    subpage(title, &view, &[link_button("Open in browser", url.to_owned()).upcast()])
}

fn loading_view(o: &Ole) -> webkit6::WebView {
    html_view("<p class=meta>Loading…</p>", &o.folder)
}

fn page_page(o: &Rc<Ole>, m: &Value) -> adw::NavigationPage {
    let view = loading_view(o);
    let (o2, v, cmid) = (o.clone(), view.clone(), m["id"].clone());
    o.call("mod_page_get_pages_by_courses", vec![("courseids[0]", o.course.to_string())], move |r| {
        let html = r.and_then(|v| arr(&v["pages"]).iter().find(|p| p["coursemodule"] == cmid).cloned().ok_or("page not found".into()));
        set_html(&v, &match html {
            Ok(p) => o2.html(&format!("<h1>{}</h1>{}{}", esc(&s(&p["name"])), s(&p["intro"]), s(&p["content"]))),
            Err(e) => format!("<p>Could not load: {}</p>", esc(&e)),
        });
    });
    subpage(&s(&m["name"]), &view, &[link_button("Open in browser", s(&m["url"])).upcast()])
}

fn forum_page(o: &Rc<Ole>, nav: &adw::NavigationView, forum: i64, name: &str) -> adw::NavigationPage {
    let holder = adw::Bin::new();
    let reload: Rc<dyn Fn()> = Rc::new({
        let (o, nav, holder) = (o.clone(), nav.clone(), holder.clone());
        move || {
            let page = adw::PreferencesPage::new();
            let g = group("");
            page.add(&g);
            holder.set_child(Some(&page));
            let (o2, nav2) = (o.clone(), nav.clone());
            let token = o.token.clone();
            fill(
                &g,
                move || call(&token, "mod_forum_get_forum_discussions", &[("forumid", forum.to_string()), ("sortorder", "-1".into()), ("page", "0".into()), ("perpage", "100".into())]),
                move |g, v| {
                    let ds = arr(&v["discussions"]);
                    if ds.is_empty() {
                        g.add(&row("No discussions yet", ""));
                    }
                    for d in ds {
                        let sub = format!("{} · {} · {} replies", s(&d["userfullname"]), fmt_ts(d["timemodified"].as_i64().unwrap_or_default()), s(&d["numreplies"]));
                        let r = nav_row(&s(&d["name"]), &sub, if d["pinned"] == true { "view-pin-symbolic" } else { "mail-read-symbolic" });
                        let (o3, nav3, id, title) = (o2.clone(), nav2.clone(), d["discussion"].as_i64().unwrap_or_default(), s(&d["name"]));
                        r.connect_activated(move |_| nav3.push(&discussion_page(&o3, id, &title)));
                        g.add(&r);
                    }
                },
            );
        }
    });
    reload();
    let new = {
        let (o, holder) = (o.clone(), holder.clone());
        button("New discussion", move |_| {
            let (o, holder, reload) = (o.clone(), holder.clone(), reload.clone());
            glib::spawn_future_local(async move {
                if let Some((subject, text)) = ask_text(&holder, "New discussion", Some(""), "").await {
                    o.call("mod_forum_add_discussion", vec![("forumid", forum.to_string()), ("subject", subject), ("message", text_to_html(&text))], move |r| match r {
                        Ok(_) => reload(),
                        Err(e) => eprintln!("post: {e}"),
                    });
                }
            });
        })
    };
    subpage(name, &holder, &[new.upcast()])
}

fn discussion_page(o: &Rc<Ole>, discussion: i64, name: &str) -> adw::NavigationPage {
    let view = loading_view(o);
    let root_post = Rc::new(std::cell::Cell::new(0i64));
    let load: Rc<dyn Fn()> = Rc::new({
        let (o, view, root_post) = (o.clone(), view.clone(), root_post.clone());
        move || {
            let (o2, view, root_post) = (o.clone(), view.clone(), root_post.clone());
            o.call("mod_forum_get_discussion_posts", vec![("discussionid", discussion.to_string())], move |r| {
                let html = r.map(|v| {
                    let mut posts = arr(&v["posts"]).to_vec();
                    posts.sort_by_key(|p| p["timecreated"].as_i64());
                    let mut h = String::new();
                    for p in &posts {
                        let is_root = p["parentid"].as_i64().unwrap_or(0) == 0;
                        if is_root {
                            root_post.set(p["id"].as_i64().unwrap_or_default());
                        }
                        let files: String = arr(&p["attachments"]).iter().map(|a| format!("<li><a href=\"{}\">{}</a></li>", esc(&s(&a["fileurl"])), esc(&s(&a["filename"])))).collect();
                        h += &format!(
                            "<article class=\"{}\"><h3>{}</h3><p class=meta>{} · {}</p>{}{}</article>",
                            if is_root { "" } else { "reply" },
                            esc(&s(&p["subject"])),
                            esc(&s(&p["author"]["fullname"])),
                            fmt_ts(p["timecreated"].as_i64().unwrap_or_default()),
                            s(&p["message"]),
                            if files.is_empty() { String::new() } else { format!("<ul>{files}</ul>") },
                        );
                    }
                    h
                });
                set_html(&view, &o2.html(&r_or_error(html)));
            });
        }
    });
    load();
    let reply = {
        let (o, view, name) = (o.clone(), view.clone(), name.to_owned());
        button("Reply", move |_| {
            let (o, view, load, root_post, name) = (o.clone(), view.clone(), load.clone(), root_post.clone(), name.clone());
            glib::spawn_future_local(async move {
                if let Some((_, text)) = ask_text(&view, &format!("Reply to “{name}”"), None, "").await {
                    let args = vec![("postid", root_post.get().to_string()), ("subject", format!("Re: {name}")), ("message", text_to_html(&text))];
                    o.call("mod_forum_add_discussion_post", args, move |r| match r {
                        Ok(_) => load(),
                        Err(e) => eprintln!("reply: {e}"),
                    });
                }
            });
        })
    };
    subpage(name, &view, &[reply.upcast()])
}

fn r_or_error(r: Res<String>) -> String {
    r.unwrap_or_else(|e| format!("<p>Could not load: {}</p>", esc(&e)))
}

struct AssignState {
    id: i64,
    plugins: Vec<String>,
    text: String,
    files: Vec<(String, String)>,
    can_edit: bool,
    can_submit: bool,
}

fn assign_page(o: &Rc<Ole>, cmid: i64, name: &str) -> adw::NavigationPage {
    let view = loading_view(o);
    let state: Rc<std::cell::RefCell<Option<AssignState>>> = Rc::default();
    let edit = gtk::Button::builder().label("Edit text").css_classes(["flat"]).visible(false).build();
    let upload = gtk::Button::builder().label("Upload file").css_classes(["flat"]).visible(false).build();
    let submit = gtk::Button::builder().label("Submit for grading").css_classes(["suggested-action"]).valign(gtk::Align::Center).visible(false).build();

    let load: Rc<dyn Fn()> = Rc::new({
        let (o, view, state, edit, upload, submit) = (o.clone(), view.clone(), state.clone(), edit.clone(), upload.clone(), submit.clone());
        move || {
            let (token, course) = (o.token.clone(), o.course);
            let (o2, view, state, edit, upload, submit) = (o.clone(), view.clone(), state.clone(), edit.clone(), upload.clone(), submit.clone());
            spawn(
                move || {
                    let all = call(&token, "mod_assign_get_assignments", &[("courseids[0]", course.to_string())])?;
                    let a = arr(&all["courses"]).iter().flat_map(|c| arr(&c["assignments"])).find(|a| a["cmid"] == cmid).cloned().ok_or("assignment not found")?;
                    let st = call(&token, "mod_assign_get_submission_status", &[("assignid", s(&a["id"]))])?;
                    Ok((a, st))
                },
                move |r: Res<(Value, Value)>| {
                    let (a, st) = match r {
                        Ok(x) => x,
                        Err(e) => return set_html(&view, &r_or_error(Err(e))),
                    };
                    let (html, astate) = render_assign(&o2, &a, &st);
                    edit.set_visible(astate.can_edit && astate.plugins.iter().any(|p| p == "onlinetext"));
                    upload.set_visible(astate.can_edit && astate.plugins.iter().any(|p| p == "file"));
                    submit.set_visible(astate.can_submit);
                    *state.borrow_mut() = Some(astate);
                    set_html(&view, &o2.html(&html));
                },
            );
        }
    });
    load();

    let ctx = (o.clone(), state.clone(), load.clone(), view.clone());
    edit.connect_clicked({
        let (o, state, load, view) = ctx.clone();
        move |_| {
            let Some((id, initial)) = state.borrow().as_ref().map(|s| (s.id, strip_html(&s.text))) else { return };
            let (o, load, view) = (o.clone(), load.clone(), view.clone());
            glib::spawn_future_local(async move {
                if let Some((_, text)) = ask_text(&view, "Online text", None, &initial).await {
                    let args = vec![("assignmentid", id.to_string()), ("plugindata[onlinetext_editor][text]", text_to_html(&text)), ("plugindata[onlinetext_editor][format]", "1".into()), ("plugindata[onlinetext_editor][itemid]", "0".into())];
                    o.call("mod_assign_save_submission", args, move |r| match r {
                        Ok(_) => load(),
                        Err(e) => eprintln!("save: {e}"),
                    });
                }
            });
        }
    });
    upload.connect_clicked({
        let (o, state, load, view) = ctx.clone();
        move |_| {
            let Some((id, existing)) = state.borrow().as_ref().map(|s| (s.id, s.files.clone())) else { return };
            let (o, load, view) = (o.clone(), load.clone(), view.clone());
            glib::spawn_future_local(async move {
                let Ok(file) = gtk::FileDialog::new().open_future(window_of(&view).as_ref()).await else { return };
                let Some(path) = file.path() else { return };
                let token = o.token.clone();
                spawn(
                    move || {
                        // Saving replaces the whole file area, so re-upload what is already there.
                        let mut item = 0;
                        for (url, name) in &existing {
                            let mut bytes = Vec::new();
                            std::io::Read::read_to_end(&mut ureq::get(&file_url(url, &token)).call().map_err(err)?.into_reader(), &mut bytes).map_err(err)?;
                            item = moodle::upload(&token, item, name, &bytes)?;
                        }
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("file".into());
                        item = moodle::upload(&token, item, &name, &std::fs::read(&path).map_err(err)?)?;
                        call(&token, "mod_assign_save_submission", &[("assignmentid", id.to_string()), ("plugindata[files_filemanager]", item.to_string())])
                    },
                    move |r| match r {
                        Ok(_) => load(),
                        Err(e) => eprintln!("upload: {e}"),
                    },
                );
            });
        }
    });
    submit.connect_clicked({
        let (o, state, load, view) = ctx;
        move |_| {
            let Some(id) = state.borrow().as_ref().map(|s| s.id) else { return };
            let (o, load, view) = (o.clone(), load.clone(), view.clone());
            glib::spawn_future_local(async move {
                if confirm(&view, "Submit for grading?", "You may not be able to change the submission afterwards.", "Submit").await {
                    o.call("mod_assign_submit_for_grading", vec![("assignmentid", id.to_string()), ("acceptsubmissionstatement", "1".into())], move |r| match r {
                        Ok(_) => load(),
                        Err(e) => eprintln!("submit: {e}"),
                    });
                }
            });
        }
    });
    subpage(name, &view, &[edit.upcast(), upload.upcast(), submit.upcast()])
}

fn render_assign(o: &Ole, a: &Value, st: &Value) -> (String, AssignState) {
    let mut h = format!("<h1>{}</h1><p class=meta>", esc(&s(&a["name"])));
    for (label, key) in [("Opens", "allowsubmissionsfromdate"), ("Due", "duedate"), ("Cut-off", "cutoffdate")] {
        if let Some(t) = a[key].as_i64().filter(|t| *t > 0) {
            h += &format!("{label}: {}<br>", fmt_ts(t));
        }
    }
    h += "</p>";
    h += &s(&a["intro"]);
    let files = |v: &Value| -> Vec<(String, String)> {
        arr(&v["fileareas"]).iter().flat_map(|fa| arr(&fa["files"])).map(|f| (s(&f["fileurl"]), s(&f["filename"]))).collect()
    };
    let links = |fs: &[(String, String)]| -> String { fs.iter().map(|(u, n)| format!("<li><a href=\"{}\">{}</a></li>", esc(&file_url(u, &o.token)), esc(n))).collect() };
    let intro_files: Vec<_> = arr(&a["introattachments"]).iter().map(|f| (s(&f["fileurl"]), s(&f["filename"]))).collect();
    if !intro_files.is_empty() {
        h += &format!("<ul>{}</ul>", links(&intro_files));
    }

    let last = &st["lastattempt"];
    let sub = if last["submission"].is_object() { &last["submission"] } else { &last["teamsubmission"] };
    let (mut text, mut sub_files) = (String::new(), Vec::new());
    h += &format!("<h2>Your submission</h2><p><span class=tag>{}</span> <span class=tag>{}</span></p>", esc(&s(&sub["status"])), esc(&s(&last["gradingstatus"])));
    for p in arr(&sub["plugins"]) {
        match s(&p["type"]).as_str() {
            "onlinetext" => {
                text = arr(&p["editorfields"]).first().map(|e| s(&e["text"])).unwrap_or_default();
                h += &text;
            }
            "file" => sub_files = files(p),
            _ => {}
        }
    }
    if !sub_files.is_empty() {
        h += &format!("<ul>{}</ul>", links(&sub_files));
    }
    let fb = &st["feedback"];
    if fb.is_object() {
        h += &format!("<h2>Feedback</h2><p>Grade: {}</p>", s(&fb["gradefordisplay"]));
        for p in arr(&fb["plugins"]) {
            for e in arr(&p["editorfields"]) {
                h += &s(&e["text"]);
            }
            let fs = files(p);
            if !fs.is_empty() {
                h += &format!("<ul>{}</ul>", links(&fs));
            }
        }
    }
    let plugins = arr(&a["configs"])
        .iter()
        .filter(|c| c["subtype"] == "assignsubmission" && c["name"] == "enabled" && s(&c["value"]) == "1")
        .map(|c| s(&c["plugin"]))
        .collect();
    let state = AssignState {
        id: a["id"].as_i64().unwrap_or_default(),
        plugins,
        text,
        files: sub_files,
        can_edit: last["canedit"].as_bool().unwrap_or(false),
        can_submit: last["cansubmit"].as_bool().unwrap_or(false),
    };
    (h, state)
}

fn grades_page(o: &Rc<Ole>) -> adw::NavigationPage {
    let view = loading_view(o);
    let (o2, v) = (o.clone(), view.clone());
    o.call("gradereport_user_get_grades_table", vec![("courseid", o.course.to_string())], move |r| {
        let html = r.map(|t| {
            let mut h = String::from("<table><tr><th>Item</th><th>Grade</th><th>Range</th><th>Percent</th><th>Feedback</th></tr>");
            for row in arr(&t["tables"][0]["tabledata"]).iter().filter(|r| r["itemname"].is_object()) {
                h += "<tr>";
                for k in ["itemname", "grade", "range", "percentage", "feedback"] {
                    h += &format!("<td>{}</td>", s(&row[k]["content"]));
                }
                h += "</tr>";
            }
            h + "</table>"
        });
        set_html(&v, &o2.html(&r_or_error(html)));
    });
    subpage("Grades", &view, &[])
}

fn participants_page(o: &Rc<Ole>, _nav: &adw::NavigationView) -> adw::NavigationPage {
    let page = adw::PreferencesPage::new();
    let g = group("");
    page.add(&g);
    let token = o.token.clone();
    let id = o.course;
    let page2 = page.clone();
    fill(&g, move || call(&token, "core_enrol_get_enrolled_users", &[("courseid", id.to_string())]), move |g, v| {
        let mut by_role: std::collections::BTreeMap<String, Vec<&Value>> = Default::default();
        for u in arr(&v) {
            let role = arr(&u["roles"]).iter().map(|r| Some(s(&r["name"])).filter(|n| !n.is_empty()).unwrap_or(s(&r["shortname"]))).collect::<Vec<_>>().join(", ");
            by_role.entry(if role.is_empty() { "No role".into() } else { role }).or_default().push(u);
        }
        page2.remove(g);
        // Teachers before students.
        let mut roles: Vec<_> = by_role.into_iter().collect();
        roles.sort_by_key(|(r, us)| (!r.to_lowercase().contains("teacher"), us.len()));
        for (role, users) in roles {
            let g = group(&format!("{role} ({})", users.len()));
            for u in users {
                let r = row(&s(&u["fullname"]), &s(&u["email"]));
                if let Some(mail) = u["email"].as_str().filter(|m| !m.is_empty()) {
                    r.set_activatable(true);
                    let mail = format!("mailto:{mail}");
                    r.connect_activated(move |r| open_uri(r, &mail));
                }
                g.add(&r);
            }
            page2.add(&g);
        }
    });
    subpage("Participants", &page, &[])
}

fn upcoming_page(o: &Rc<Ole>, nav: &adw::NavigationView) -> adw::NavigationPage {
    let page = adw::PreferencesPage::new();
    let g = group("");
    page.add(&g);
    let (token, id, o2, nav2) = (o.token.clone(), o.course, o.clone(), nav.clone());
    fill(&g, move || call(&token, "core_calendar_get_action_events_by_course", &[("courseid", id.to_string())]), move |g, v| {
        let evs = arr(&v["events"]);
        if evs.is_empty() {
            g.add(&row("Nothing due", ""));
        }
        for e in evs {
            let r = nav_row(&s(&e["name"]), &fmt_ts(e["timesort"].as_i64().unwrap_or_default()), "alarm-symbolic");
            let (o3, nav3, url, name) = (o2.clone(), nav2.clone(), s(&e["url"]), s(&e["name"]));
            r.connect_activated(move |_| nav3.push(&web_page(&o3, &url, &name)));
            g.add(&r);
        }
    });
    subpage("Upcoming deadlines", &page, &[])
}
