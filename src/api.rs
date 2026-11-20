//! Blocking fetchers for mySNS, OLE (Moodle WS) and Microsoft Graph, plus course matching.
//! Call these from gio::spawn_blocking.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use gtk::glib;
use serde_json::Value;

use crate::auth::{Config, OLE, Res, err, user_of};

const WS: &str = "https://webservices.scientificnet.org/rest/cm";
const API: &str = "https://api.scientificnet.org";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

#[derive(Clone)]
pub struct Access {
    pub cockpit: String,
    pub exams: Res<String>,
    pub graph: Res<String>,
    pub moodle: Res<String>,
}

#[derive(Clone, Default)]
pub struct Lecture {
    pub start: String,
    pub end: String,
    pub activity: String,
    pub room: String,
    pub staff: String,
}

#[derive(Clone, Default)]
pub struct Course {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub credits: f64,
    pub status: String,
    pub mark: Option<i64>,
    pub cum_laude: bool,
    pub exam_date: Option<String>,
    pub degree_year: i64,
    pub has_history: bool,
    pub syllabus: Option<String>,
    pub lectures: Vec<Lecture>,
    pub exams: Vec<Value>,
    /// (OLE course id, name)
    pub moodle: Vec<(i64, String)>,
    /// (group id, name, Teams web link)
    pub teams: Vec<(String, String, String)>,
}

pub struct Data {
    pub student_id: i64,
    pub query: String,
    pub courses: Vec<Course>,
    pub warnings: Vec<String>,
}

/// A downloadable file or a link, shown as one row.
#[derive(Clone)]
pub struct Item {
    pub title: String,
    pub subtitle: String,
    /// (url, file name); url needs no extra auth.
    pub download: Option<(String, String)>,
    pub web: Option<String>,
}

fn get(url: &str, bearer: Option<&str>) -> Res<Value> {
    let mut req = ureq::get(url);
    if let Some(t) = bearer {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    match req.call() {
        Ok(r) => r.into_json().map_err(err),
        Err(ureq::Error::Status(c, r)) => Err(format!("{c} from {}: {}", url.split('?').next().unwrap_or(url), r.into_string().unwrap_or_default().chars().take(300).collect::<String>())),
        Err(e) => Err(err(e)),
    }
}

fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.trim().to_owned(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn norm(x: &str) -> String {
    x.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Candidates whose name equals `name`; if none, those where one name contains the other.
fn pick<'a, T>(name: &str, cands: &'a [(String, T)]) -> Vec<&'a T> {
    let n = norm(name);
    let exact: Vec<_> = cands.iter().filter(|(c, _)| norm(c) == n).map(|(_, t)| t).collect();
    if !exact.is_empty() || n.is_empty() {
        return exact;
    }
    cands
        .iter()
        .filter(|(c, _)| {
            let c = norm(c);
            !c.is_empty() && (c.contains(&n) || n.contains(&c))
        })
        .map(|(_, t)| t)
        .collect()
}

fn query_param(u: &str, key: &str) -> Option<String> {
    url::Url::parse(u).ok()?.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

fn moodle(token: &str, func: &str, args: &[(&str, &str)]) -> Res<Value> {
    let mut u = url::Url::parse(&format!("{OLE}/webservice/rest/server.php")).unwrap();
    u.query_pairs_mut().append_pair("wstoken", token).append_pair("wsfunction", func).append_pair("moodlewsrestformat", "json").extend_pairs(args);
    let v = get(u.as_str(), None)?;
    // Moodle reports errors with HTTP 200.
    if v.get("exception").is_some() {
        if v["errorcode"] == "invalidtoken" {
            crate::auth::forget_moodle();
        }
        return Err(format!("OLE: {}", s(&v["message"])));
    }
    Ok(v)
}

pub fn load(a: &Access, cfg: &Config) -> Res<Data> {
    let (user, domain) = user_of(&a.cockpit)?;
    let q = format!("username={user}&domain={domain}");
    let ud = get(&format!("{WS}/core/api/Userdata/GetUserdata?{q}"), Some(&a.cockpit))?;
    let sid = ud["StudentId"].as_i64().ok_or("mySNS returned no StudentId")?;
    let career = get(&format!("{API}/studyplan-course-selection/api/Student/{sid}/career?{q}"), Some(&a.cockpit))?;

    let mut courses = Vec::new();
    let mut seen = BTreeSet::new();
    for x in arr(&career["groups"]).iter().flat_map(|g| arr(&g["exams"])) {
        let id = x["id"].as_i64().unwrap_or_default();
        if !seen.insert(id) {
            continue;
        }
        let name = Some(s(&x["descriptionEn"])).filter(|n| !n.is_empty()).unwrap_or_else(|| s(&x["description"]));
        courses.push(Course {
            id,
            code: s(&x["code"]),
            name,
            credits: x["creditPoints"].as_f64().unwrap_or_default(),
            status: s(&x["status"]["descriptionEn"]),
            mark: x["mark"].as_i64(),
            cum_laude: x["cumLaude"].as_bool().unwrap_or_default(),
            exam_date: x["examDate"].as_str().map(|d| d[..10.min(d.len())].to_owned()),
            degree_year: x["degreeYear"].as_i64().unwrap_or_default(),
            has_history: x["hasHistory"].as_bool().unwrap_or_default(),
            syllabus: x["syllabusLink"]["url"].as_str().map(Into::into),
            ..Default::default()
        });
    }

    let mut warnings = Vec::new();
    let mut warn = |what: &str, e: String| warnings.push(format!("{what}: {e}"));

    // Timetable for the current academic year, grouped by course name.
    let mut team_urls: HashMap<String, String> = HashMap::new();
    let mut by_course: BTreeMap<String, (Vec<Lecture>, BTreeSet<i64>, BTreeSet<String>)> = BTreeMap::new();
    match timetable(a, &q) {
        Ok(entries) => {
            for e in entries {
                let entry = by_course.entry(s(&e["CourseDescription"])).or_default();
                let staff = arr(&e["Staff"]);
                for m in std::iter::once(&e["MoodleUrl"]).chain(staff.iter().map(|p| &p["MoodleUrl"])) {
                    if let Some(id) = m.as_str().and_then(|u| query_param(u, "id")).and_then(|i| i.parse().ok()) {
                        entry.1.insert(id);
                    }
                }
                for p in staff {
                    if let Some((u, g)) = p["TeamsUrl"].as_str().and_then(|u| Some((u, query_param(u, "groupId")?))) {
                        team_urls.insert(g.clone(), u.to_owned());
                        entry.2.insert(g);
                    }
                }
                entry.0.push(Lecture {
                    start: s(&e["StartDate"]),
                    end: s(&e["EndDate"]),
                    activity: s(&e["ActivityDescription"]),
                    room: s(&e["RoomDescription"]),
                    staff: staff.iter().map(|p| format!("{} {}", s(&p["Firstname"]), s(&p["Lastname"]))).collect::<Vec<_>>().join(", "),
                });
            }
        }
        Err(e) => warn("Timetable", e),
    }
    let tt: Vec<(String, &(Vec<Lecture>, BTreeSet<i64>, BTreeSet<String>))> = by_course.iter().map(|(k, v)| (k.clone(), v)).collect();

    let exams: Vec<Value> = match &a.exams {
        Ok(t) => match get(&format!("{API}/exam-enrolment-evaluation//api/student/{sid}/exams?{q}"), Some(t)) {
            Ok(v) => arr(&v["activeGroup"]).iter().chain(arr(&v["inactiveGroup"])).cloned().collect(),
            Err(e) => (warn("Exams", e), vec![]).1,
        },
        Err(e) => (warn("Exams login", e.clone()), vec![]).1,
    };

    let ole: Vec<(String, (i64, String))> = match &a.moodle {
        Ok(t) => match moodle(t, "core_webservice_get_site_info", &[]).and_then(|si| moodle(t, "core_enrol_get_users_courses", &[("userid", &s(&si["userid"]))])) {
            Ok(v) => arr(&v).iter().map(|c| (s(&c["fullname"]), (c["id"].as_i64().unwrap_or_default(), s(&c["fullname"])))).collect(),
            Err(e) => (warn("OLE", e), vec![]).1,
        },
        Err(e) => (warn("OLE login", e.clone()), vec![]).1,
    };
    let ole_names: HashMap<i64, String> = ole.iter().map(|(_, c)| c.clone()).collect();

    let teams: Vec<(String, (String, String))> = match &a.graph {
        Ok(t) => match get(&format!("{GRAPH}/me/joinedTeams?$select=id,displayName"), Some(t)) {
            Ok(v) => arr(&v["value"]).iter().map(|t| (s(&t["displayName"]), (s(&t["id"]), s(&t["displayName"])))).collect(),
            Err(e) => (warn("Teams list", e), vec![]).1,
        },
        Err(e) => (warn("Microsoft Graph login", e.clone()), vec![]).1,
    };
    let team_names: HashMap<String, String> = teams.iter().map(|(_, t)| t.clone()).collect();

    for c in &mut courses {
        let mut m_ids = BTreeSet::new();
        let mut t_ids = BTreeSet::new();
        for (lect, m, t) in pick(&c.name, &tt) {
            c.lectures.extend(lect.iter().cloned());
            m_ids.extend(m);
            t_ids.extend(t.iter().cloned());
        }
        c.lectures.sort_by(|a, b| a.start.cmp(&b.start));
        // Fall back to name matching only when the timetable carried no links.
        if m_ids.is_empty() {
            m_ids.extend(pick(&c.name, &ole).into_iter().map(|(id, _)| *id));
        }
        if t_ids.is_empty() {
            t_ids.extend(pick(&c.name, &teams).into_iter().map(|(id, _)| id.clone()));
        }
        if let Some(o) = cfg.overrides.get(&c.code) {
            m_ids = o.moodle.iter().copied().collect();
            t_ids = o.teams.iter().cloned().collect();
        }
        c.moodle = m_ids.into_iter().map(|id| (id, ole_names.get(&id).cloned().unwrap_or(format!("OLE course {id}")))).collect();
        c.teams = t_ids
            .into_iter()
            .map(|id| {
                let url = team_urls.get(&id).cloned().unwrap_or("https://teams.microsoft.com/".into());
                (id.clone(), team_names.get(&id).cloned().unwrap_or("Team".into()), url)
            })
            .collect();
        c.exams = exams
            .iter()
            .filter(|e| {
                let code = s(&e["examCode"]);
                if code.is_empty() { norm(&s(&e["examDescription"])) == norm(&c.name) } else { code == c.code }
            })
            .cloned()
            .collect();
    }
    // Current courses first, then open ones, then passed.
    courses.sort_by_key(|c| (c.lectures.is_empty() && c.exams.is_empty(), c.status == "Passed", c.degree_year, c.name.clone()));
    Ok(Data { student_id: sid, query: q, courses, warnings })
}

fn timetable(a: &Access, q: &str) -> Res<Vec<Value>> {
    let now = glib::DateTime::now_utc().map_err(err)?;
    let year = if now.month() >= 9 { now.year() } else { now.year() - 1 };
    let mut from = glib::DateTime::from_utc(year, 9, 1, 0, 0, 0.0).map_err(err)?;
    let mut out = Vec::new();
    // 4-week windows: the web app only ever asks for one week, so keep requests small.
    for _ in 0..13 {
        let to = from.add_days(28).map_err(err)?;
        let f = |d: &glib::DateTime| d.format("%Y-%m-%dT%H:%M:%S.000Z").map(|s| s.to_string()).map_err(err);
        let v = get(&format!("{WS}/student/api/v2/Timetable?from={}&to={}&{q}", f(&from)?, f(&to)?), Some(&a.cockpit))?;
        out.extend(arr(&v).iter().cloned());
        from = to;
    }
    Ok(out)
}

/// Previous exam attempts: (date, result).
pub fn history(a: &Access, sid: i64, q: &str, course_id: i64) -> Res<Vec<(String, String)>> {
    let v = get(&format!("{API}/studyplan-course-selection/api/Student/{sid}/history/{course_id}?{q}"), Some(&a.cockpit))?;
    Ok(arr(&v["examSessionEnrollments"]).iter().map(|e| (s(&e["examDate"]).chars().take(10).collect(), s(&e["result"]["descriptionEn"]))).collect())
}

/// OLE course contents: (section name, items).
pub fn moodle_contents(a: &Access, course: i64) -> Res<Vec<(String, Vec<Item>)>> {
    let token = a.moodle.clone()?;
    let v = moodle(&token, "core_course_get_contents", &[("courseid", &course.to_string())])?;
    let mut out = Vec::new();
    for sec in arr(&v) {
        let mut items = Vec::new();
        for m in arr(&sec["modules"]) {
            let (name, kind, contents) = (s(&m["name"]), s(&m["modname"]), arr(&m["contents"]));
            let files: Vec<_> = contents.iter().filter(|f| f["type"] == "file").collect();
            if kind == "url" {
                let link = contents.first().map(|c| s(&c["fileurl"])).filter(|u| !u.is_empty()).or(m["url"].as_str().map(Into::into));
                items.push(Item { title: name, subtitle: "Link".into(), download: None, web: link });
            } else if files.is_empty() || kind == "folder" && files.len() > 20 {
                items.push(Item { title: name, subtitle: kind, download: None, web: m["url"].as_str().map(Into::into) });
            } else {
                let sub = if files.len() == 1 { kind.clone() } else { format!("{name} · {kind}") };
                for f in files {
                    let u = s(&f["fileurl"]);
                    let u = format!("{u}{}token={token}", if u.contains('?') { '&' } else { '?' });
                    items.push(Item { title: s(&f["filename"]), subtitle: sub.clone(), download: Some((u, s(&f["filename"]))), web: m["url"].as_str().map(Into::into) });
                }
            }
        }
        if !items.is_empty() {
            out.push((s(&sec["name"]), items));
        }
    }
    Ok(out)
}

/// Every file in a team's SharePoint drive (channel folders included).
pub fn team_files(a: &Access, group: &str) -> Res<Vec<Item>> {
    let token = a.graph.clone()?;
    let mut out = Vec::new();
    walk(&token, &format!("{GRAPH}/groups/{group}/drive/root/children"), group, "", 0, &mut out)?;
    out.sort_by(|a, b| (&a.subtitle, &a.title).cmp(&(&b.subtitle, &b.title)));
    Ok(out)
}

fn walk(token: &str, url: &str, group: &str, path: &str, depth: u32, out: &mut Vec<Item>) -> Res<()> {
    let mut next = Some(url.to_owned());
    while let Some(u) = next {
        let v = get(&u, Some(token))?;
        for it in arr(&v["value"]) {
            let name = s(&it["name"]);
            if it.get("folder").is_some() {
                if depth < 6 {
                    walk(token, &format!("{GRAPH}/groups/{group}/drive/items/{}/children", s(&it["id"])), group, &format!("{path}{name}/"), depth + 1, out)?;
                }
            } else {
                out.push(Item {
                    subtitle: if path.is_empty() { "/".into() } else { path.trim_end_matches('/').into() },
                    download: it["@microsoft.graph.downloadUrl"].as_str().map(|d| (d.to_owned(), name.clone())),
                    web: it["webUrl"].as_str().map(Into::into),
                    title: name,
                });
            }
        }
        next = v["@odata.nextLink"].as_str().map(Into::into);
    }
    Ok(())
}

/// Downloads to ~/Downloads/unibz/<course>/<file> and returns the path.
pub fn download(url: &str, course: &str, file: &str) -> Res<std::path::PathBuf> {
    let clean = |x: &str| x.replace(['/', '\0'], "_");
    let dir = glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(glib::home_dir).join("unibz").join(clean(course));
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(clean(file));
    let resp = ureq::get(url).call().map_err(err)?;
    let mut f = std::fs::File::create(&path).map_err(err)?;
    std::io::copy(&mut resp.into_reader(), &mut f).map_err(err)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_exact_then_contains() {
        let c = vec![("Data Analysis".to_string(), 1), ("Analysis".to_string(), 2), ("Network Security".to_string(), 3)];
        assert_eq!(pick("analysis", &c), vec![&2]);
        assert_eq!(pick("Network-Security 2025/26", &c), vec![&3]);
        assert!(pick("Databases", &c).is_empty());
    }

    #[test]
    fn teams_group_id_from_url() {
        let u = "https://teams.microsoft.com/l/team/19%3aX%40thread.tacv2/conversations?groupId=287a6024-ac0b&tenantId=9251";
        assert_eq!(query_param(u, "groupId").as_deref(), Some("287a6024-ac0b"));
    }
}
