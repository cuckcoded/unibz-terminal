# unibz terminal

GTK4/libadwaita app that combines mySNS (grades, exam sessions, timetable), OLE (Moodle) and Microsoft Teams files, organized by course.

```sh
cargo run
```

Needs gtk4 ≥ 4.14, libadwaita ≥ 1.6 and webkitgtk-6.0.

## Sign-in

You sign in in embedded browser windows, and the cookies are kept, so later sign-ins complete on their own:

1. **mySNS**: uses mySNS's own Microsoft client.
2. **OLE**: uses the Moodle mobile-app flow and stores a web-service token.
3. **Microsoft Graph**: uses the Microsoft Office public client, because mySNS's client can only read your profile.

Refresh tokens are kept in `~/.config/unibz-terminal/tokens.json` (mode 0600).

## Course matching

1. Every mySNS timetable entry carries its OLE course id and Teams group id. These are used first.
2. If a course has neither, it is matched to OLE courses and teams by name.
3. To set the matches by hand, edit `~/.config/unibz-terminal/config.json`:

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

Downloads go to `~/Downloads/unibz/<course>/`.
