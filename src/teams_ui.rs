//! Teams tab: channels (posts, tabs, files) and the team's SharePoint libraries, via Microsoft Graph.
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use serde_json::{Value, json};

use crate::api::{self, Dl, GRAPH, Item, arr, s};
use crate::auth::Res;
use crate::ui::*;

pub struct Team {
    pub token: String,
    pub id: String,
    pub name: String,
    pub url: String,
    pub folder: String,
}

impl Team {
    fn get(&self, path: &str) -> Res<Value> {
        api::get(&format!("{GRAPH}{path}"), Some(&self.token))
    }
    /// All pages of a collection.
    fn list(&self, path: &str) -> Res<Vec<Value>> {
        let mut out = Vec::new();
        let mut v = self.get(path)?;
        loop {
            out.extend(arr(&v["value"]).iter().cloned());
            match v["@odata.nextLink"].as_str() {
                Some(next) => v = api::get(next, Some(&self.token))?,
                None => return Ok(out),
            }
        }
    }
    fn post(&self, path: &str, body: Value) -> Res<Value> {
        api::post_json(&format!("{GRAPH}{path}"), Some(&self.token), &body)
    }
}

/// Runs `job` with a Send copy of the team context.
fn with<T: Send + 'static>(t: &Rc<Team>, job: impl FnOnce(&Team) -> Res<T> + Send + 'static) -> impl FnOnce() -> Res<T> + Send + 'static {
    let copy = Team { token: t.token.clone(), id: t.id.clone(), name: t.name.clone(), url: t.url.clone(), folder: t.folder.clone() };
    move || job(&copy)
}

/// `actions` go into the team page header (e.g. the assign menu).
pub fn tab(t: Rc<Team>, actions: &[gtk::Widget]) -> adw::NavigationView {
    let nav = adw::NavigationView::new();
    let page = WidePage::new();

    let channels = group("Channels");
    channels.set_header_suffix(Some(&link_button("Open in Teams", t.url.clone())));
    let id = t.id.clone();
    let (t2, nav2) = (t.clone(), nav.clone());
    fill(&channels, with(&t, move |t| t.list(&format!("/teams/{id}/channels"))), move |g, chs| {
        for ch in chs {
            let kind = s(&ch["membershipType"]);
            let sub = [s(&ch["description"]), if kind == "standard" { String::new() } else { kind }].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            let r = nav_row(&s(&ch["displayName"]), &sub, "user-available-symbolic");
            let (t3, nav3) = (t2.clone(), nav2.clone());
            r.connect_activated(move |_| nav3.push(&channel_page(&t3, &nav3, &ch)));
            g.add(&r);
        }
    });
    page.add(&channels);

    let sp = group("SharePoint");
    sp.set_description(Some("Document libraries of the team site"));
    let id = t.id.clone();
    let (t2, nav2) = (t.clone(), nav.clone());
    fill(
        &sp,
        with(&t, move |t| {
            let site = t.get(&format!("/groups/{id}/sites/root?$select=webUrl,displayName"))?;
            Ok((site, t.list(&format!("/groups/{id}/drives"))?))
        }),
        move |g, (site, drives)| {
            if let Some(u) = site["webUrl"].as_str() {
                let r = row("Team site", u);
                r.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
                r.set_activatable(true);
                let u = u.to_owned();
                r.connect_activated(move |r| open_uri(r, &u));
                g.add(&r);
            }
            for d in drives {
                let r = nav_row(&s(&d["name"]), &s(&d["description"]), "folder-documents-symbolic");
                let (t3, nav3, drive, name, web) = (t2.clone(), nav2.clone(), s(&d["id"]), s(&d["name"]), s(&d["webUrl"]));
                r.connect_activated(move |_| nav3.push(&folder_page(&t3, &nav3, &drive, "root", &name, &web)));
                g.add(&r);
            }
        },
    );
    page.add(&sp);

    nav.push(&subpage(&t.name, &page.root, actions));
    nav
}

fn human_size(b: i64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} kB", b >> 10),
        b => format!("{b} B"),
    }
}

/// Adds rows for drive items: folders navigate, files download.
fn drive_rows(t: &Rc<Team>, nav: &adw::NavigationView, g: &adw::PreferencesGroup, items: Vec<Value>) {
    if items.is_empty() {
        g.add(&row("Empty folder", ""));
    }
    let drive_of = |it: &Value| s(&it["parentReference"]["driveId"]);
    let mut items = items;
    items.sort_by_key(|it| (it.get("folder").is_none(), s(&it["name"]).to_lowercase()));
    for it in items {
        let name = s(&it["name"]);
        if it.get("folder").is_some() {
            let r = nav_row(&name, &format!("{} items", s(&it["folder"]["childCount"])), "folder-symbolic");
            let (t2, nav2, drive, id, web) = (t.clone(), nav.clone(), drive_of(&it), s(&it["id"]), s(&it["webUrl"]));
            r.connect_activated(move |_| nav2.push(&folder_page(&t2, &nav2, &drive, &id, &name, &web)));
            g.add(&r);
        } else {
            let sub = format!(
                "{} · {} · {}",
                human_size(it["size"].as_i64().unwrap_or_default()),
                s(&it["lastModifiedBy"]["user"]["displayName"]),
                fmt_iso(&s(&it["lastModifiedDateTime"]))
            );
            let item = Item {
                title: name.clone(),
                subtitle: sub,
                download: it["@microsoft.graph.downloadUrl"].as_str().map(|d| {
                    // parentReference.path looks like "/drives/{id}/root:/General/Slides".
                    let parent = s(&it["parentReference"]["path"]);
                    let parent = parent.split_once("root:").map_or("", |(_, p)| p);
                    Dl { url: d.to_owned(), rel: format!("Teams/{}/{parent}/{name}", t.name), version: s(&it["eTag"]) }
                }),
                web: it["webUrl"].as_str().map(Into::into),
            };
            g.add(&item_row(&item, &t.folder));
        }
    }
}

fn folder_page(t: &Rc<Team>, nav: &adw::NavigationView, drive: &str, item: &str, name: &str, web: &str) -> adw::NavigationPage {
    let page = adw::PreferencesPage::new();
    let g = group("");
    page.add(&g);
    let path = if item == "root" { format!("/drives/{drive}/root/children") } else { format!("/drives/{drive}/items/{item}/children") };
    let (t2, nav2) = (t.clone(), nav.clone());
    fill(&g, with(t, move |t| t.list(&path)), move |g, items| drive_rows(&t2, &nav2, g, items));
    subpage(name, &page, &[link_button("Open in browser", web.to_owned()).upcast()])
}

fn channel_page(t: &Rc<Team>, nav: &adw::NavigationView, ch: &Value) -> adw::NavigationPage {
    let (team, cid, cname) = (t.id.clone(), s(&ch["id"]), s(&ch["displayName"]));
    let holder = adw::Bin::new();
    let reload: Rc<dyn Fn()> = Rc::new({
        let (t, nav, holder, team, cid) = (t.clone(), nav.clone(), holder.clone(), team.clone(), cid.clone());
        move || {
            let page = adw::PreferencesPage::new();
            holder.set_child(Some(&page));

            let posts = group("Posts");
            let (t2, nav2, team2, cid2) = (t.clone(), nav.clone(), team.clone(), cid.clone());
            let path = format!("/teams/{team}/channels/{cid}/messages?$top=30");
            fill(&posts, with(&t, move |t| t.get(&path)), move |g, v| {
                let msgs: Vec<_> = arr(&v["value"]).iter().filter(|m| m["messageType"] == "message" && m["deletedDateTime"].is_null()).collect();
                if msgs.is_empty() {
                    g.add(&row("No posts", ""));
                }
                for m in msgs {
                    let preview: String = strip_html(&s(&m["body"]["content"])).lines().next().unwrap_or("").chars().take(140).collect();
                    let title = Some(s(&m["subject"])).filter(|x| !x.is_empty()).unwrap_or(preview.clone());
                    let sub = format!("{} · {}", s(&m["from"]["user"]["displayName"]), fmt_iso(&s(&m["createdDateTime"])));
                    let r = nav_row(&title, &sub, "mail-unread-symbolic");
                    let (t3, nav3, team3, cid3, mid) = (t2.clone(), nav2.clone(), team2.clone(), cid2.clone(), s(&m["id"]));
                    r.connect_activated(move |_| nav3.push(&thread_page(&t3, &team3, &cid3, &mid, &title)));
                    g.add(&r);
                }
            });

            let files = group("Files");
            let (t2, nav2) = (t.clone(), nav.clone());
            let path = format!("/teams/{team}/channels/{cid}/filesFolder");
            fill(
                &files,
                with(&t, move |t| {
                    let f = t.get(&path)?;
                    let drive = s(&f["parentReference"]["driveId"]);
                    t.list(&format!("/drives/{drive}/items/{}/children", s(&f["id"])))
                }),
                move |g, items| drive_rows(&t2, &nav2, g, items),
            );

            let tabs = group("Tabs");
            let path = format!("/teams/{team}/channels/{cid}/tabs?$expand=teamsApp");
            fill(&tabs, with(&t, move |t| t.list(&path)), move |g, tabs| {
                if tabs.is_empty() {
                    g.add(&row("No tabs", ""));
                }
                for tab in tabs {
                    let c = &tab["configuration"];
                    let url = [&c["websiteUrl"], &c["contentUrl"], &tab["webUrl"]].into_iter().map(s).find(|u| u.starts_with("http"));
                    let r = row(&s(&tab["displayName"]), &s(&tab["teamsApp"]["displayName"]));
                    r.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
                    if let Some(u) = url {
                        r.set_activatable(true);
                        r.connect_activated(move |r| open_uri(r, &u));
                    }
                    g.add(&r);
                }
            });

            page.add(&posts);
            page.add(&files);
            page.add(&tabs);
        }
    });
    reload();
    let new = {
        let (t, holder) = (t.clone(), holder.clone());
        button("New post", move |_| {
            let (t, holder, reload, path) = (t.clone(), holder.clone(), reload.clone(), format!("/teams/{team}/channels/{cid}/messages"));
            glib::spawn_future_local(async move {
                if let Some((subject, text)) = ask_text(&holder, "New post", Some(""), "").await {
                    let body = json!({ "subject": subject, "body": { "contentType": "html", "content": text_to_html(&text) } });
                    spawn(with(&t, move |t| t.post(&path, body)), move |r| match r {
                        Ok(_) => reload(),
                        Err(e) => eprintln!("post: {e}"),
                    });
                }
            });
        })
    };
    subpage(&cname, &holder, &[new.upcast()])
}

fn message_html(m: &Value, reply: bool) -> String {
    let body = s(&m["body"]["content"]);
    let body = if m["body"]["contentType"] == "html" { body } else { text_to_html(&body) };
    let atts: String = arr(&m["attachments"])
        .iter()
        .filter(|a| a["contentType"] == "reference")
        .map(|a| format!("<li><a href=\"{}\">{}</a></li>", esc(&s(&a["contentUrl"])), esc(&s(&a["name"]))))
        .collect();
    format!(
        "<article class=\"{}\">{}<p class=meta>{} · {}</p>{}{}</article>",
        if reply { "reply" } else { "" },
        Some(s(&m["subject"])).filter(|x| !x.is_empty()).map(|x| format!("<h3>{}</h3>", esc(&x))).unwrap_or_default(),
        esc(&s(&m["from"]["user"]["displayName"])),
        fmt_iso(&s(&m["createdDateTime"])),
        body,
        if atts.is_empty() { String::new() } else { format!("<ul>{atts}</ul>") }
    )
}

fn thread_page(t: &Rc<Team>, team: &str, channel: &str, msg: &str, title: &str) -> adw::NavigationPage {
    let view = html_view("<p class=meta>Loading…</p>", &t.folder);
    let base = format!("/teams/{team}/channels/{channel}/messages/{msg}");
    let load: Rc<dyn Fn()> = Rc::new({
        let (t, view, base) = (t.clone(), view.clone(), base.clone());
        move || {
            let (view, base) = (view.clone(), base.clone());
            spawn(
                with(&t, move |t| {
                    let root = t.get(&base)?;
                    let mut replies = t.list(&format!("{base}/replies"))?;
                    replies.sort_by_key(|r| s(&r["createdDateTime"]));
                    let mut h = message_html(&root, false);
                    for r in replies.iter().filter(|r| r["deletedDateTime"].is_null()) {
                        h += &message_html(r, true);
                    }
                    Ok(h)
                }),
                move |r| set_html(&view, &r.unwrap_or_else(|e| format!("<p>Could not load: {}</p>", esc(&e)))),
            );
        }
    });
    load();
    let reply = {
        let (t, view) = (t.clone(), view.clone());
        button("Reply", move |_| {
            let (t, view, load, path) = (t.clone(), view.clone(), load.clone(), format!("{base}/replies"));
            glib::spawn_future_local(async move {
                if let Some((_, text)) = ask_text(&view, "Reply", None, "").await {
                    let body = json!({ "body": { "contentType": "html", "content": text_to_html(&text) } });
                    spawn(with(&t, move |t| t.post(&path, body)), move |r| match r {
                        Ok(_) => load(),
                        Err(e) => eprintln!("reply: {e}"),
                    });
                }
            });
        })
    };
    subpage(title, &view, &[reply.upcast()])
}
