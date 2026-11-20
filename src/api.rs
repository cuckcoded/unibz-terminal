//! Blocking HTTP helpers, mySNS loading and course matching.
//! Call these from gio::spawn_blocking.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use gtk::glib;
use serde_json::Value;

use crate::auth::{Config, Res, err, user_of};
use crate::moodle;

const WS: &str = "https://webservices.scientificnet.org/rest/cm";
const API: &str = "https://api.scientificnet.org";
pub const GRAPH: &str = "https://graph.microsoft.com/v1.0";

#[derive(Clone)]
pub struct Access {
    pub cockpit: String,
    pub exams: Res<String>,
    pub graph: Res<String>,
    pub moodle: Res<String>,
}

#[derive(Clone, Default)]
pub struct Lecture {
    /// Timetable ActivityCode: LECT, LAB or EXAM.
    pub kind: String,
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
    /// OLE courses the timetable links to this course that you are not enrolled in.
    pub suggest_moodle: Vec<(i64, String)>,
    /// Teams the timetable links to this course that you have not joined: (group id, name, web link, join code).
    pub suggest_teams: Vec<(String, String, String, String)>,
}

impl Course {
    pub fn passed(&self) -> bool {
        self.mark.is_some() || self.status == "Passed"
    }

    /// Earliest exam not yet over, from the timetable (EXAM entries) or open exam sessions:
    /// (local ISO start, description). `now` is local "YYYY-MM-DDTHH:MM:SS".
    pub fn next_exam(&self, now: &str) -> Option<(String, String)> {
        let timetable = self.lectures.iter().filter(|l| l.kind == "EXAM" && l.end.as_str() >= now).map(|l| (l.start.clone(), [l.activity.clone(), l.room.clone()].join(" · ")));
        let sessions = self.exams.iter().filter_map(|e| {
            let date = s(&e["examDate"]).get(..10)?.to_owned();
            let time = s(&e["examTime"]);
            let start = format!("{date}T{}:00", time.get(..5).filter(|t| t.contains(':')).unwrap_or("23:59"));
            let desc = [s(&e["examTypeDescription"]), s(&e["location"])].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            (start.as_str() >= now).then_some((start, desc))
        });
        timetable.chain(sessions).min_by(|a, b| a.0.cmp(&b.0))
    }
}

pub struct Data {
    pub student_id: i64,
    pub query: String,
    pub courses: Vec<Course>,
    pub warnings: Vec<String>,
    /// OLE user id (for autologin) and every enrolled OLE course (id, name).
    pub ole_user: Option<i64>,
    pub ole: Vec<(i64, String)>,
    /// Every joined team (group id, name, web link).
    pub teams: Vec<(String, String, String)>,
}

/// A downloadable file or a link, shown as one row.
#[derive(Clone)]
pub struct Item {
    pub title: String,
    pub subtitle: String,
    pub download: Option<Dl>,
    pub web: Option<String>,
}

/// A file to keep in the local course folder.
#[derive(Clone)]
pub struct Dl {
    /// Needs no extra auth.
    pub url: String,
    /// Path inside the course folder, e.g. "Week 1/Slides/intro.pdf".
    pub rel: String,
    /// Changes whenever the remote file changes (timestamp, size, eTag…).
    pub version: String,
}

fn send(req: ureq::Request, bearer: Option<&str>, f: impl FnOnce(ureq::Request) -> Result<ureq::Response, ureq::Error>) -> Res<Value> {
    let url = req.url().split('?').next().unwrap_or_default().to_owned();
    let req = match bearer {
        Some(t) => req.set("Authorization", &format!("Bearer {t}")),
        None => req,
    };
    match f(req) {
        // Some endpoints answer 201/204 with an empty body.
        Ok(r) => Ok(serde_json::from_str(&r.into_string().map_err(err)?).unwrap_or(Value::Null)),
        Err(ureq::Error::Status(c, r)) => Err(format!("{c} from {url}: {}", r.into_string().unwrap_or_default().chars().take(300).collect::<String>())),
        Err(e) => Err(err(e)),
    }
}

pub fn get(url: &str, bearer: Option<&str>) -> Res<Value> {
    send(ureq::get(url), bearer, |r| r.call())
}

pub fn post_form(url: &str, bearer: Option<&str>, form: &[(&str, &str)]) -> Res<Value> {
    send(ureq::post(url), bearer, |r| r.send_form(form))
}

pub fn post_json(url: &str, bearer: Option<&str>, body: &Value) -> Res<Value> {
    send(ureq::post(url), bearer, |r| r.send_json(body))
}

pub fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.trim().to_owned(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

pub fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

pub fn norm(x: &str) -> String {
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
    let mut team_codes: HashMap<String, String> = HashMap::new();
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
                        if let Some(code) = p["TeamsJoinCode"].as_str().or(e["TeamsJoinCode"].as_str()) {
                            team_codes.insert(g.clone(), code.to_owned());
                        }
                        entry.2.insert(g);
                    }
                }
                entry.0.push(Lecture {
                    kind: s(&e["ActivityCode"]).to_uppercase(),
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

    let (mut ole_user, mut ole_known, mut teams_known) = (None, false, false);
    let ole: Vec<(String, (i64, String))> = match &a.moodle {
        Ok(t) => match moodle::call(t, "core_webservice_get_site_info", &[]).and_then(|si| {
            ole_user = si["userid"].as_i64();
            moodle::call(t, "core_enrol_get_users_courses", &[("userid", s(&si["userid"]))])
        }) {
            Ok(v) => {
                ole_known = true;
                arr(&v).iter().map(|c| (s(&c["fullname"]), (c["id"].as_i64().unwrap_or_default(), s(&c["fullname"])))).collect()
            }
            Err(e) => (warn("OLE", e), vec![]).1,
        },
        Err(e) => (warn("OLE login", e.clone()), vec![]).1,
    };
    let ole_names: HashMap<i64, String> = ole.iter().map(|(_, c)| c.clone()).collect();

    let teams: Vec<(String, (String, String))> = match &a.graph {
        Ok(t) => match get(&format!("{GRAPH}/me/joinedTeams?$select=id,displayName,webUrl"), Some(t)) {
            Ok(v) => arr(&v["value"])
                .iter()
                .inspect(|_| teams_known = true)
                .map(|t| {
                    if let Some(u) = t["webUrl"].as_str() {
                        team_urls.entry(s(&t["id"])).or_insert(u.to_owned());
                    }
                    (s(&t["displayName"]), (s(&t["id"]), s(&t["displayName"])))
                })
                .collect(),
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
        // Timetable links you can't open yet become suggestions (only when the enrolment lists loaded).
        let mut sugg_m = BTreeSet::new();
        let mut sugg_t = BTreeSet::new();
        if ole_known {
            (m_ids, sugg_m) = m_ids.into_iter().partition(|id| ole_names.contains_key(id));
        }
        if teams_known {
            (t_ids, sugg_t) = t_ids.into_iter().partition(|id| team_names.contains_key(id));
        }
        if let Some(o) = cfg.overrides.get(&c.code) {
            m_ids = o.moodle.iter().copied().collect();
            t_ids = o.teams.iter().cloned().collect();
        }
        c.suggest_moodle = sugg_m.into_iter().filter(|id| !m_ids.contains(id)).map(|id| (id, format!("OLE course {id}"))).collect();
        c.suggest_teams = sugg_t
            .into_iter()
            .filter(|id| !t_ids.contains(id))
            .map(|id| (id.clone(), "Team".into(), team_urls.get(&id).cloned().unwrap_or_default(), team_codes.get(&id).cloned().unwrap_or_default()))
            .collect();
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
    name_suggestions(a, &mut courses);
    // Current courses first, then open ones, then passed.
    courses.sort_by_key(|c| (c.lectures.is_empty() && c.exams.is_empty(), c.status == "Passed", c.degree_year, c.name.clone()));
    let teams = teams.into_iter().map(|(_, (id, name))| {
        let url = team_urls.get(&id).cloned().unwrap_or("https://teams.microsoft.com/".into());
        (id, name, url)
    });
    Ok(Data { student_id: sid, query: q, courses, warnings, ole_user, ole: ole.into_iter().map(|(_, c)| c).collect(), teams: teams.collect() })
}

/// Looks up names of suggested courses you can't see in your own lists. Best effort.
fn name_suggestions(a: &Access, courses: &mut [Course]) {
    let ids: BTreeSet<i64> = courses.iter().flat_map(|c| c.suggest_moodle.iter().map(|m| m.0)).collect();
    let mut names: HashMap<String, String> = HashMap::new();
    if let (Ok(t), false) = (&a.moodle, ids.is_empty()) {
        let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        if let Ok(v) = moodle::call(t, "core_course_get_courses_by_field", &[("field", "ids".into()), ("value", list)]) {
            names.extend(arr(&v["courses"]).iter().map(|c| (s(&c["id"]), s(&c["fullname"]))));
        }
    }
    let groups: BTreeSet<String> = courses.iter().flat_map(|c| c.suggest_teams.iter().map(|t| t.0.clone())).collect();
    if let Ok(t) = &a.graph {
        for g in groups {
            if let Ok(v) = get(&format!("{GRAPH}/groups/{g}?$select=displayName"), Some(t)) {
                names.insert(g, s(&v["displayName"]));
            }
        }
    }
    for c in courses {
        for m in &mut c.suggest_moodle {
            if let Some(n) = names.get(&m.0.to_string()) {
                m.1 = n.clone();
            }
        }
        for t in &mut c.suggest_teams {
            if let Some(n) = names.get(&t.0) {
                t.1 = n.clone();
            }
        }
    }
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

fn course_dir(folder: &str) -> std::path::PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(glib::home_dir).join("unibz").join(clean_component(folder))
}

fn clean_component(x: &str) -> String {
    let c = x.trim().replace(['/', '\\', '\0'], "_");
    if c.is_empty() || c == "." || c == ".." { "_".into() } else { c }
}

/// Safe relative path: every component cleaned, so nothing escapes the course folder.
fn clean_rel(rel: &str) -> std::path::PathBuf {
    rel.split('/').filter(|p| !p.trim().is_empty()).map(clean_component).collect()
}

/// Per-course-folder index of saved files: rel path -> version.
static INDEX: std::sync::Mutex<Option<HashMap<std::path::PathBuf, HashMap<String, String>>>> = std::sync::Mutex::new(None);
const INDEX_FILE: &str = ".unibz-cache.json";

fn with_index<T>(dir: &std::path::Path, f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut all = INDEX.lock().unwrap();
    let idx = all
        .get_or_insert_with(HashMap::new)
        .entry(dir.to_path_buf())
        .or_insert_with(|| std::fs::read(dir.join(INDEX_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default());
    f(idx)
}

/// Whether this exact version is already saved locally.
pub fn is_cached(folder: &str, dl: &Dl) -> bool {
    let dir = course_dir(folder);
    let rel = clean_rel(&dl.rel);
    dir.join(&rel).is_file() && with_index(&dir, |i| i.get(&*rel.to_string_lossy()) == Some(&dl.version))
}

/// Returns the local copy, downloading only if missing or outdated.
pub fn fetch_file(folder: &str, dl: &Dl) -> Res<std::path::PathBuf> {
    let dir = course_dir(folder);
    let rel = clean_rel(&dl.rel);
    let path = dir.join(&rel);
    if is_cached(folder, dl) {
        return Ok(path);
    }
    std::fs::create_dir_all(path.parent().unwrap_or(&dir)).map_err(err)?;
    // Download next to the target and rename, so an interrupted download never replaces a good copy.
    let part = path.with_extension("part-unibz");
    let resp = ureq::get(&dl.url).call().map_err(err)?;
    std::io::copy(&mut resp.into_reader(), &mut std::fs::File::create(&part).map_err(err)?).map_err(err)?;
    std::fs::rename(&part, &path).map_err(err)?;
    with_index(&dir, |i| {
        i.insert(rel.to_string_lossy().into_owned(), dl.version.clone());
        std::fs::write(dir.join(INDEX_FILE), serde_json::to_vec(i).unwrap_or_default()).map_err(err)
    })?;
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
    fn next_exam_picks_earliest_upcoming() {
        let lect = |kind: &str, start: &str| Lecture { kind: kind.into(), start: start.into(), end: start.into(), ..Default::default() };
        let c = Course {
            lectures: vec![lect("EXAM", "2026-01-10T09:00:00"), lect("LECT", "2026-01-05T09:00:00"), lect("EXAM", "2026-02-10T09:00:00")],
            exams: vec![serde_json::json!({"examDate": "2026-01-20T00:00:00", "examTime": "14:00"})],
            ..Default::default()
        };
        assert_eq!(c.next_exam("2026-01-01T00:00:00").unwrap().0, "2026-01-10T09:00:00");
        assert_eq!(c.next_exam("2026-01-11T00:00:00").unwrap().0, "2026-01-20T14:00:00");
        assert!(c.next_exam("2026-03-01T00:00:00").is_none());
    }

    #[test]
    fn clean_rel_stays_inside() {
        assert_eq!(clean_rel("../a//b/../c.pdf"), std::path::PathBuf::from("_/a/b/_/c.pdf"));
    }

    #[test]
    fn teams_group_id_from_url() {
        let u = "https://teams.microsoft.com/l/team/19%3aX%40thread.tacv2/conversations?groupId=287a6024-ac0b&tenantId=9251";
        assert_eq!(query_param(u, "groupId").as_deref(), Some("287a6024-ac0b"));
    }
}
