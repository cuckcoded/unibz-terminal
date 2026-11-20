//! OLE (Moodle) web-service client. Blocking: call from gio::spawn_blocking.
use serde_json::Value;

use crate::api::{post_form, s};
use crate::auth::{OLE, Res, err};

pub fn call(token: &str, func: &str, args: &[(&str, String)]) -> Res<Value> {
    let url = format!("{OLE}/webservice/rest/server.php?moodlewsrestformat=json&wsfunction={func}");
    let mut form: Vec<(&str, &str)> = vec![("wstoken", token)];
    form.extend(args.iter().map(|(k, v)| (*k, v.as_str())));
    let v = post_form(&url, None, &form)?;
    // Moodle reports errors with HTTP 200.
    if v.get("exception").is_some() {
        if v["errorcode"] == "invalidtoken" {
            crate::auth::forget_moodle();
        }
        return Err(format!("OLE: {}", s(&v["message"])));
    }
    Ok(v)
}

/// Web-service file URLs need the token appended.
pub fn file_url(url: &str, token: &str) -> String {
    if !url.contains("/webservice/pluginfile.php") || url.contains("token=") {
        return url.to_owned();
    }
    format!("{url}{}token={token}", if url.contains('?') { '&' } else { '?' })
}

/// Appends the token to every webservice/pluginfile.php URL inside HTML.
pub fn fix_html(html: &str, token: &str) -> String {
    let needle = format!("{OLE}/webservice/pluginfile.php");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(&needle) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(['"', '\'', ' ', '<', '>', ')']).unwrap_or(tail.len());
        out.push_str(&file_url(&tail[..end].replace("&amp;", "&"), token));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Uploads a file to the user's draft area; pass itemid 0 to start a new area.
pub fn upload(token: &str, itemid: i64, filename: &str, bytes: &[u8]) -> Res<i64> {
    let boundary = format!("----unibz{}", gtk::glib::random_int());
    let mut body = Vec::new();
    for (k, v) in [("token", token.to_owned()), ("filearea", "draft".into()), ("itemid", itemid.to_string())] {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    let fname = filename.replace(['"', '\r', '\n'], "_");
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file_1\"; filename=\"{fname}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    let r = ureq::post(&format!("{OLE}/webservice/upload.php"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .map_err(err)?;
    let v: Value = r.into_json().map_err(err)?;
    v[0]["itemid"].as_i64().ok_or_else(|| format!("upload failed: {}", s(&v["error"])))
}

/// A URL that logs the embedded browser into OLE, then continues to `url`.
pub fn autologin(token: &str, private: Option<&str>, user: Option<i64>, url: &str) -> String {
    let (Some(p), Some(user)) = (private, user) else { return url.to_owned() };
    match call(token, "tool_mobile_get_autologin_key", &[("privatetoken", p.to_owned())]) {
        Ok(v) => {
            let mut u = match url::Url::parse(&s(&v["autologinurl"])) {
                Ok(u) => u,
                Err(_) => return url.to_owned(),
            };
            u.query_pairs_mut().append_pair("userid", &user.to_string()).append_pair("key", &s(&v["key"])).append_pair("urltogo", url);
            u.into()
        }
        // Rate-limited (one key per few minutes); the browser session usually already exists.
        Err(_) => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fix_html_appends_token() {
        let h = r#"<img src="https://ole.unibz.it/webservice/pluginfile.php/1/a.png"> <a href='https://ole.unibz.it/webservice/pluginfile.php/2/b.pdf?forcedownload=1&amp;x=1'>"#;
        let out = super::fix_html(h, "T");
        assert!(out.contains("a.png?token=T\""));
        assert!(out.contains("b.pdf?forcedownload=1&x=1&token=T'"));
    }
}
