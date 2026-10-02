# Orbit Bot

A personal Discord bot written in Rust that manages tasks in
[Orbit](./orbit-api.md). You install it on **your own Discord account** (not on
a server), so you can use it in any server, DM, or group DM. Only the Discord
users listed in `ALLOWED_USER_IDS` can use it; everyone else gets
"This is a private bot."

## Commands

All commands are under `/task`. Replies are ephemeral (only you see them) and
use Discord Components V2 cards. A task card shows the title (a link to Orbit),
a short description, status, priority, assignee, project, deadline,
sub-issues, image attachments, and an
**Update Status** menu that changes the status in place.

| Command | What it does |
|---|---|
| `/task add` | Open a form: title, and project, priority, assignee, and label dropdowns. The task starts in the project's first to-do status. |
| `/task list [project] [status] [closed]` | List tasks, most urgent first, 8 per page with Previous/Next buttons. |
| `/task this-week [assignee] [status] [project]` | Open tasks due this week (Monday to Sunday), most urgent first, with the same pages. |
| `/task view <task>` | Show one task, with a link to it in Orbit. |
| `/task status <task> <status>` | Change a task's status. |
| `/task done <task>` | Move a task to its project's first completed status. |
| `/task delete <task>` | Delete a task (Orbit keeps it in the trash for 30 days). |

- **task**: start typing a title and pick from the suggestions. You can also
  type the code Orbit shows (e.g. `PERSO-4A5B`) or an exact title.
- **project**: suggestions show `Name (KEY)`. The `/task add` form preselects
  `ORBIT_DEFAULT_PROJECT`, or the only project if there is one.
- **status**: suggestions show the statuses of the chosen project or task.
- **assignee**: suggestions show the workspace members.
- **priority**: Urgent, High, Medium, Low, None (default).

## How it talks to Orbit

Orbit's REST API accepts only a signed-in user session (API tokens can only
create tasks from Discord messages). The bot signs in as its own Orbit user with
`ORBIT_EMAIL` / `ORBIT_PASSWORD`, keeps the session cookie in memory, and signs
in again when the session expires. Projects, statuses, and labels are cached for
60 seconds. Details: [`orbit-api.md`](./orbit-api.md).

## Setup

1. **Orbit bot user** (one time): in Orbit, open **Settings → Members → Invite a
   member**, invite an email for the bot as **Member**, open the invitation
   link in a private window, and set a display name and password
   (see `orbit-api.md` §3.2.1).
2. **Discord application** in the
   [Developer Portal](https://discord.com/developers/applications):
   - **Installation** → **Installation Contexts**: enable **User Install** and
     disable **Guild Install**. Install Link: "Discord Provided Link".
   - **Bot**: copy the token. No privileged intents are needed.
   - Open the install link and choose **Add to My Apps**.
3. **Configuration**: `task setup` (or copy `.env.example` to `.env`) and fill in
   `DISCORD_TOKEN`, `ALLOWED_USER_IDS`, `ORBIT_URL`, `ORBIT_EMAIL`,
   `ORBIT_PASSWORD`. Optional: `ORBIT_WORKSPACE_ID`, `ORBIT_DEFAULT_PROJECT`,
   `TIMEZONE`, `RUST_LOG`.
4. **Icons** (optional): upload the PNGs in [`icons/`](./icons) in the
   Developer Portal → your app → **Emojis**, then restart the bot (it reads
   the emojis by name at start-up). Anything without an icon uses a Unicode
   stand-in.
   - **Status**: `status_unstarted`, `status_started`, `status_completed`,
     `status_cancelled`, `status_duplicate` (one icon per category).
   - **Priority**: `priority_urgent` … `priority_none`.
   Keep the file names when you upload.
   - **Assignee**: an emoji named after the member's Orbit display name in
     lower case, e.g. `shadowarcanist`.
   - Regenerate the icons with `python3 scripts/make_icons.py`.

## Patched serenity

`vendor/serenity` is serenity 0.12.5 with one small patch so the bot can read
dropdowns in modals. See [`vendor/README.md`](./vendor/README.md).

## Run and test

With [go-task](https://taskfile.dev) the `.env` file is loaded automatically:

```sh
task check-orbit   # sign in to Orbit and list workspace, projects, statuses
task dev           # run the bot (debug build)
task start         # run the bot (release build)
task test          # unit tests
task check         # format check + clippy
```

Without go-task: `cargo run -- check`, `cargo run`, `cargo test`.

On start the log shows the Orbit workspace, the number of allowed users,
`registered 1 command(s)`, and `starting the bot`. The first global command
registration can take some minutes to show in Discord.
