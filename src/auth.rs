//! Logins: Microsoft (mySNS + Graph) via auth-code/PKCE in an embedded WebKit view,
//! and OLE (Moodle) via the mobile-app launch flow. Refresh tokens persist in
//! ~/.config/unibz-terminal/tokens.json (mode 0600).
use std::{cell::RefCell, collections::HashMap, io::Read, path::PathBuf, rc::Rc, sync::Mutex};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use webkit6::prelude::*;

pub type Res<T> = Result<T, String>;
pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const TENANT: &str = "92513267-03e3-401a-80d4-c58ed6674e3b";
pub const OLE: &str = "https://ole.unibz.it";
pub const COCKPIT: &str = "https://scientificnet.onmicrosoft.com/CockpitMobileTest/user_impersonation";
pub const EXAMS: &str = "https://scientificnet.onmicrosoft.com/mysns-exam-enrolment-evaluation-api/access_as_user";
pub const GRAPH: &str = "https://graph.microsoft.com/.default";

#[derive(Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub moodle_token: Option<String>,
    pub graph_client_id: Option<String>,
    pub graph_redirect_uri: Option<String>,
    /// mySNS course code -> manual OLE/Teams assignment (replaces auto-matching).
    pub overrides: HashMap<String, Override>,
}

#[derive(Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Override {
    pub moodle: Vec<i64>,
    pub teams: Vec<String>,
}

#[derive(Clone)]
pub struct Client {
    key: &'static str,
    id: String,
    redirect: String,
    /// SPA clients only redeem codes when the request carries their origin.
    origin: Option<&'static str>,
}

/// The mySNS web app's own client registration.
pub fn sns() -> Client {
    Client {
        key: "sns",
        id: "aaf3f968-94f2-454f-9d58-aa4571e1c880".into(),
        redirect: "https://my.scientificnet.org/home/en/".into(),
        origin: Some("https://my.scientificnet.org"),
    }
}

/// mySNS's client only has User.Read on Graph, so Teams files need another client.
/// Default: Microsoft Office's public client (pre-consented for Graph in most tenants).
pub fn graph(cfg: &Config) -> Client {
    Client {
        key: "graph",
        id: cfg.graph_client_id.clone().unwrap_or("d3590ed6-52b3-4102-aeff-aad2292ab01c".into()),
        redirect: cfg.graph_redirect_uri.clone().unwrap_or("urn:ietf:wg:oauth:2.0:oob".into()),
        origin: None,
    }
}

fn dir() -> PathBuf {
    glib::user_config_dir().join("unibz-terminal")
}

pub fn config() -> Config {
    std::fs::read(dir().join("config.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| eprintln!("config.json: {e}")).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &Config) -> Res<()> {
    std::fs::create_dir_all(dir()).map_err(err)?;
    std::fs::write(dir().join("config.json"), serde_json::to_vec_pretty(cfg).map_err(err)?).map_err(err)
}

static TOKENS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn stored(key: &str) -> Option<String> {
    let mut t = TOKENS.lock().unwrap();
    t.get_or_insert_with(|| {
        std::fs::read(dir().join("tokens.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    })
    .get(key)
    .cloned()
}

fn store(key: &str, val: Option<String>) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut t = TOKENS.lock().unwrap();
    let map = t.get_or_insert_with(HashMap::new);
    match val {
        Some(v) => map.insert(key.into(), v),
        None => map.remove(key),
    };
    let _ = std::fs::create_dir_all(dir());
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(dir().join("tokens.json"));
    if let Err(e) = file.and_then(|f| serde_json::to_writer(f, map).map_err(Into::into)) {
        eprintln!("saving tokens: {e}");
    }
}

fn random_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("/dev/urandom");
    URL_SAFE_NO_PAD.encode(b)
}

/// Access token for `scope`: silent refresh first, interactive login if that fails.
pub async fn access_token(c: &Client, scope: &str) -> Res<String> {
    if let Some(rt) = stored(c.key) {
        let (c2, s) = (c.clone(), scope.to_owned());
        match gio::spawn_blocking(move || redeem(&c2, &s, &[("grant_type", "refresh_token"), ("refresh_token", &rt)])).await.map_err(|_| "panic")? {
            Ok(t) => return Ok(t),
            Err(e) => eprintln!("{} refresh failed, logging in again: {e}", c.key),
        }
    }
    let verifier = random_b64(32);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier));
    let mut url = url::Url::parse(&format!("https://login.microsoftonline.com/{TENANT}/oauth2/v2.0/authorize")).unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", &c.id)
        .append_pair("response_type", "code")
        .append_pair("response_mode", "query")
        .append_pair("redirect_uri", &c.redirect)
        .append_pair("scope", &format!("{scope} openid profile offline_access"))
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let back = webview_login("Sign in with your unibz account", url.as_str(), &c.redirect).await?;
    let back = url::Url::parse(&back).map_err(err)?;
    let params: HashMap<String, String> = back.query_pairs().chain(url::form_urlencoded::parse(back.fragment().unwrap_or("").as_bytes())).map(|(k, v)| (k.into(), v.into())).collect();
    let code = params.get("code").cloned().ok_or_else(|| format!("login failed: {}", params.get("error_description").map_or("no code returned", |s| s)))?;
    let (c2, s) = (c.clone(), scope.to_owned());
    gio::spawn_blocking(move || {
        redeem(&c2, &s, &[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", &c2.redirect), ("code_verifier", &verifier)])
    })
    .await
    .map_err(|_| "panic")?
}

fn redeem(c: &Client, scope: &str, grant: &[(&str, &str)]) -> Res<String> {
    let scope = format!("{scope} openid profile offline_access");
    let mut form = vec![("client_id", c.id.as_str()), ("scope", &scope)];
    form.extend_from_slice(grant);
    let mut req = ureq::post(&format!("https://login.microsoftonline.com/{TENANT}/oauth2/v2.0/token"));
    if let Some(o) = c.origin {
        req = req.set("Origin", o);
    }
    let v: serde_json::Value = match req.send_form(&form) {
        Ok(r) => r.into_json().map_err(err)?,
        Err(ureq::Error::Status(code, r)) => {
            if code == 400 {
                store(c.key, None);
            }
            return Err(format!("token endpoint {code}: {}", r.into_string().unwrap_or_default()));
        }
        Err(e) => return Err(err(e)),
    };
    if let Some(rt) = v["refresh_token"].as_str() {
        store(c.key, Some(rt.into()));
    }
    v["access_token"].as_str().map(Into::into).ok_or("no access_token in response".into())
}

/// (username, domain) from a token's `upn`, e.g. evandini@unibz.it -> ("evandini", "unibz").
pub fn user_of(token: &str) -> Res<(String, String)> {
    let payload = token.split('.').nth(1).ok_or("malformed token")?;
    let v: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).map_err(err)?).map_err(err)?;
    let upn = v["upn"].as_str().or(v["unique_name"].as_str()).ok_or("token has no upn")?;
    let (user, host) = upn.split_once('@').ok_or("bad upn")?;
    Ok((user.to_lowercase(), host.split('.').next().unwrap_or("unibz").into()))
}

/// OLE web-service token, from config.json, the store, or the mobile-app SSO launch flow.
pub async fn moodle_token(cfg: &Config) -> Res<String> {
    if let Some(t) = cfg.moodle_token.clone().or_else(|| stored("moodle")) {
        return Ok(t);
    }
    let passport = glib::random_int();
    let url = format!("{OLE}/admin/tool/mobile/launch.php?service=moodle_mobile_app&passport={passport}&urlscheme=moodlemobile");
    let back = webview_login("Sign in to OLE", &url, "moodlemobile://").await?;
    let b64 = back.split("token=").nth(1).ok_or("OLE returned no token")?.trim_end_matches('/');
    let decoded = String::from_utf8(STANDARD.decode(b64).map_err(err)?).map_err(err)?;
    // Format: sitehash:::token[:::privatetoken]
    let mut parts = decoded.split(":::").skip(1);
    let token = parts.next().ok_or("unexpected OLE token format")?.to_owned();
    store("moodle", Some(token.clone()));
    // The private token lets the embedded OLE browser log in automatically.
    store("moodle_private", parts.next().map(Into::into));
    Ok(token)
}

pub fn forget_moodle() {
    store("moodle", None);
    store("moodle_private", None);
}

/// A WebView sharing the login cookies of the sign-in windows.
pub fn web_view() -> webkit6::WebView {
    SESSION.with(|s| webkit6::WebView::builder().network_session(s).build())
}

thread_local! {
    static SESSION: webkit6::NetworkSession = {
        let (data, cache) = (dir().join("webkit"), glib::user_cache_dir().join("unibz-terminal"));
        let s = webkit6::NetworkSession::new(data.to_str(), cache.to_str());
        // Persist Microsoft/OLE SSO cookies so later logins complete without typing.
        if let Some(cm) = s.cookie_manager() {
            cm.set_persistent_storage(data.join("cookies.sqlite").to_str().unwrap(), webkit6::CookiePersistentStorage::Sqlite);
        }
        s
    };
}

/// Shows a login window and resolves with the first navigated URL starting with `prefix`.
async fn webview_login(title: &str, url: &str, prefix: &str) -> Res<String> {
    let (tx, rx) = futures_channel::oneshot::channel::<String>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let view = web_view();
    let win = gtk::Window::builder().title(title).default_width(520).default_height(720).modal(true).child(&view).build();
    if let Some(parent) = gio::Application::default().and_downcast::<gtk::Application>().and_then(|a| a.active_window()) {
        win.set_transient_for(Some(&parent));
    }
    let (prefix, weak, tx2) = (prefix.to_owned(), win.downgrade(), tx.clone());
    view.connect_decide_policy(move |_, decision, _| {
        let uri = decision
            .downcast_ref::<webkit6::NavigationPolicyDecision>()
            .and_then(|d| d.navigation_action())
            .and_then(|a| a.request())
            .and_then(|r| r.uri());
        match uri {
            Some(uri) if uri.starts_with(&prefix) => {
                decision.ignore();
                if let Some(tx) = tx2.borrow_mut().take() {
                    let _ = tx.send(uri.into());
                }
                if let Some(w) = weak.upgrade() {
                    w.close();
                }
                true
            }
            _ => false,
        }
    });
    win.connect_close_request(move |_| {
        tx.borrow_mut().take(); // dropping the sender cancels the login
        glib::Propagation::Proceed
    });
    view.load_uri(url);
    win.present();
    rx.await.map_err(|_| "login window closed".into())
}
