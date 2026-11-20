# unibz terminal

GTK4/libadwaita app that combines mySNS (grades, exam sessions, timetable), OLE (Moodle) and Microsoft Teams files, organized by course.

```sh
./install.sh              # build and install to ~/.local (adds an app menu entry)
./install.sh --uninstall  # remove it; keeps config, tokens and downloaded files
cargo run                 # or run straight from the source tree
```

Needs gtk4 ≥ 4.14, libadwaita ≥ 1.6 and webkitgtk-6.0.

## Sign-in

You sign in in embedded browser windows, and the cookies are kept, so later sign-ins complete on their own:

1. **mySNS**: uses mySNS's own Microsoft client.
2. **OLE**: uses the Moodle mobile-app flow and stores a web-service token.
3. **Microsoft Graph**: uses the Microsoft Office public client, because mySNS's client can only read your profile.

Refresh tokens are kept in `~/.config/unibz-terminal/tokens.json` (mode 0600).

## Layout

- **Home:** your profile, plus stat cards for ECTS progress, weighted average, projected base grade, passed exams, next exam and this week's lectures. It also lists the week's lectures, upcoming exams, OLE deadlines across all courses, recent results and any loading problems.
- **Calendar:**
  - Day, week, month and year views.
  - It shows mySNS lectures and exams and OLE calendar events (deadlines, course and personal events), color-coded by source.
  - Events are cards showing time, course, type, room and lecturer. A lunch bar (12:00–13:30) separates morning and afternoon and opens the Mensa page.
  - Click an event to open its course. Click a day in the month or year view to open it.
- **Due soon:**
  - OLE assignments due within 24 hours that you haven't submitted or completed are shown in red, both on Home and in the calendar.
  - Among the Home stats, a pill lists these assignments.
- **File preview:**
  - In a course page, opening a file (OLE resource or folder, Teams/SharePoint file, attachment in a post) shows it in a pane on the right instead of another app.
  - PDFs, images, text and code, HTML, audio and video show directly. Word, PowerPoint, Excel and OpenDocument files are converted to PDF with LibreOffice, if it's installed.
  - The pane has buttons to open the file in another app and to close the preview.
- **Wide screens:** Home, course overviews and the OLE and Teams course tabs place their sections in up to three columns as the window gets wider.
- **Home lectures:** Home lists one day of lectures at a time, with buttons for the previous day, today and the next day.
- **Mensa:**
  - Your credit, read from the mensa reserved area. Sign-in uses the "SNS Access UNIBZ" button with your existing Microsoft session. The credit also appears on Home.
  - The menu, one day at a time. Buttons move to the previous day, today or the next day. Dropdowns pick lunch or dinner and the location (Bolzano, Bressanone or NOI).
  - One column per dish type (first courses, second courses, sides…), each dish with its photo and name. Hover a dish for its allergens; click it to open its page.
- **Sidebar folders:**
  - **This semester:** courses with lectures in the last two weeks or later, or with exam sessions.
  - **Year 1, 2, 3:** the rest of your study plan.
  - **Other OLE courses** and **Other teams:** everything not linked to a mySNS course.
- **Course page tabs:**
  - **mySNS:** grade, attempts, exam sessions, lectures and linked courses.
  - **OLE:** one tab per linked OLE course. Files, pages, forums (read, reply, new discussion), assignments (status, feedback, online text, file upload, submitting), completion checkboxes, grades, participants and deadlines work natively. Other activities, like quizzes, open the real OLE page, signed in, inside the app.
  - **Teams:** one tab per linked team, with channels (posts, replies, new posts, channel tabs, files) and the team's SharePoint libraries.

## Course matching

1. Every mySNS timetable entry carries its OLE course id and Teams group id. These are used first.
2. If a course has neither, it is matched to OLE courses and teams by name.
   Timetable links to OLE courses you are not enrolled in, or teams you have not joined, appear under **Suggested by mySNS**. **Enrol** self-enrols you on OLE and asks for an enrolment key if one is needed. **Join** tries to add you to the team, and otherwise shows the join code from mySNS.
3. To change a match, use **Assign…** or the unlink buttons on the mySNS tab, or the **Assign** menu on any OLE or Teams tab. The menu can assign to another course, move to another course, or unlink. Changes are saved to `~/.config/unibz-terminal/config.json`, which you can also edit by hand:

```json
{
  "moodle_token": "optional existing OLE token",
  "graph_client_id": "optional, your own Entra app id",
  "graph_redirect_uri": "optional",
  "overrides": {
    "76268": { "moodle": [13543, 13326], "teams": ["7e92ac6a-fead-44bb-99c3-8df40b9357db"] }
  }
}
```

The keys under `overrides` are mySNS course codes. Each course page shows its code.

Opened files are kept in `~/Downloads/unibz/<course>/` (same folder structure as on OLE/Teams) and only downloaded again when they change. OLE courses have “Save all files offline”.
