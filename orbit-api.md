# Orbit — HTTP API Reference (for the orbit-bot Discord bot)

A complete reference for the Orbit REST API. It is written for a developer
(human or AI agent) who rebuilds the orbit-bot Discord bot against Orbit. You do
not need the Orbit source code to use it.

- Source analysed: Orbit at commit `2246c7c`
  (2026-10-01). Orbit is marked **"PRETTY ALPHA"**; the API can change.
- Every fact here comes from the server code and its tests. Items that could not
  be confirmed are marked **(unverified)**.
- §12 maps calls of the old PM API to the new calls.

---

## Table of contents

1. [Read this first: the two ways in](#1-read-this-first-the-two-ways-in)
2. [Base URL & configuration](#2-base-url--configuration)
3. [Authentication](#3-authentication)
4. [Conventions](#4-conventions)
5. [Error format](#5-error-format)
6. [Data model](#6-data-model)
7. [Endpoint reference — bot-critical](#7-endpoint-reference--bot-critical)
8. [Endpoint reference — workspace administration](#8-endpoint-reference--workspace-administration)
9. [Endpoint reference — secondary areas](#9-endpoint-reference--secondary-areas)
10. [Bot recipes](#10-bot-recipes)
11. [Behaviour notes & edge cases](#11-behaviour-notes--edge-cases)
12. [Migration map: old PM API → Orbit](#12-migration-map-old-pm-api--orbit)

---

## 1. Read this first: the two ways in

Orbit is **not** an open API like the old PM server. Every request is
authenticated, and there are **two different credential types** with very
different reach:

| Credential | How you get it | What it can call |
|---|---|---|
| **API token** `orb_…` (`Authorization: Bearer`) | Owner/admin creates it in **Settings → API tokens** | **Only** `POST /api/v1/integrations/discord/events` (scope `write`) and the read-only MCP server `POST /mcp` (scope `read`). |
| **Session cookie** (`__Host-orbit_session` / `orbit_session_dev`) | `POST /api/v1/auth/login` with a user's email + password | **Every other `/api/v1/...` route**: tasks, projects, statuses, labels, comments, attachments, members, realtime, … |

Consequences for the bot (confirmed in code: `task_routes.rs::authenticate_session`
reads only the cookie; `openapi.rs` gives `bearerAuth` only to `create_discord_event`):

- **"Create a task from a Discord message"** → API token + §7.1. Simple, safe,
  idempotent.
- **List, mark done, change status, delete, autocomplete, set priority/labels/due
  date** → you need a **session cookie** for a real Orbit user (recommended: a
  dedicated bot user, e.g. `orbit-bot@yourdomain`, invited as `member`). There is no
  token-based way to do these.
- **Read-only** listing *can* also go through MCP with a `read` token (§9.6), but
  the projection is reduced and it is JSON-RPC, not REST. Prefer the cookie REST
  API if the bot already has a session.

Mutating requests with a cookie session **must also send an `Origin` header**
equal to Orbit's public origin (§3.4). The Discord endpoint is exempt.

---

## 2. Base URL & configuration

| Item | Value |
|---|---|
| API prefix | `/api/v1` (plus `/health/*`, `/mcp`, `/oauth/*`, `/.well-known/oauth-*`) |
| Production default bind | `127.0.0.1:8080` (Docker `EXPOSE 8080`; example config binds `0.0.0.0:8080`) |
| Dev (`just dev`) | API on `:8080`, Vite web UI on **`:8888`** with an `/api` proxy. Public origin = `http://127.0.0.1:8888`. Call the bot against `http://127.0.0.1:8888`. |
| Web task link | `{origin}/tasks/{task_id}?workspace={workspace_id}` (React route `tasks/:taskId`) |

Server settings that change client behaviour (env var ↔ TOML key in
`config/orbit.toml`):

| Env var | Effect on API clients |
|---|---|
| `ORBIT__ENVIRONMENT` | `development` or `production`. **Production** requires an `https://` public origin, a trusted proxy, and **every request must arrive via a trusted proxy with `X-Forwarded-Proto: https`**, else `400 https_required`. Do not call the internal port directly in production. Production cookie: `__Host-orbit_session` (Secure). Development cookie: `orbit_session_dev`, and any well-formed `http(s)` Origin is accepted. |
| `ORBIT__HTTP__PUBLIC_ORIGIN` | The exact value the `Origin` header must have on cookie mutations. |
| `ORBIT__HTTP__TRUSTED_PROXIES` | CIDRs. `X-Forwarded-For/-Proto`, `X-Request-Id` are honoured only from these. |
| `ORBIT__RATE_LIMITS__*_PER_MINUTE` | §4.8. |
| `ORBIT__UPLOADS__MAX_FILE_BYTES` / `MAX_REQUEST_BYTES` | Default 25 MiB per file / 100 MiB per request (also the global body cap). |

**OpenAPI spec:** not served over HTTP. Export it with
`orbit openapi --output openapi.json`. A generated copy is in the Orbit repo at
`apps/web/src/api/generated/openapi.json`.

Optional header `X-Orbit-Contract: orbit-api-v1`. If you send it with any other
value you get `409 contract_mismatch`. Omitting it is fine.

---

## 3. Authentication

### 3.1 API tokens

**Create** — `POST /api/v1/workspaces/{workspace_id}/api-tokens`
(cookie session, workspace `owner` or `admin`; a `member` gets `403 workspace_action_forbidden`).
Normally you do this in the web UI: **Settings → API tokens**.

```json
{
  "name": "Orbit Bot",
  "service_account": true,
  "expires_in_days": 365,
  "project_ids": ["0199a1b2-0000-7000-8000-000000000001"],
  "scopes": ["write"]
}
```

| Field | Rules |
|---|---|
| `name` | Required, trimmed, 1–100 chars. |
| `service_account` | Optional, default `false`. `true` → the token acts as a service account with this name (case-insensitive; reused if one exists). Tasks it creates have `creator_id: null` and `creator_service_account_id/_name` set. It keeps working if the creating user leaves. **Recommended for a bot.** |
| `expires_in_days` | `7`, `30`, `90`, `365`, or `null`/omitted = never. Other values → `422 invalid_api_token`. |
| `project_ids` (and legacy `project_id`) | At least one. Merged and deduplicated. Each must be a live project in the workspace. **Scopes are per token and cannot be edited later**: to add a project, create a new token. |
| `scopes` | Non-empty array of `"read"` and/or `"write"`. |

Response `201` (`Cache-Control: no-store`). **`token` is shown only once.**

```json
{
  "id": "0199…", "name": "Orbit Bot", "token_prefix": "orb_AbCdEfGh",
  "scopes": ["write"],
  "project_id": "0199…", "project_ids": ["0199…"],
  "service_account_id": "0199…", "service_account_name": "Orbit Bot",
  "expires_at": "2027-10-01T12:00:00.000Z", "created_at": "2026-10-01T12:00:00.000Z",
  "last_used_at": null,
  "token": "orb_<43 base64url chars>"
}
```

- Format: `orb_` + base64url (no padding) of 32 random bytes = 47 chars. Only a
  SHA-256 hash is stored.
- `GET …/api-tokens` lists non-revoked tokens (owner/admin). `DELETE …/api-tokens/{token_id}` → `204`.
- Use: `Authorization: Bearer orb_…` (scheme is case-sensitive on the Discord route).
- A token is rejected (`401 invalid_api_token`) when it is revoked or expired, its
  workspace is deleted, a personal token's creator is suspended or removed, a
  service account is disabled, or none of its projects is still live. Checks run
  on every request (no cache). A missing scope → `403 api_token_scope_forbidden`.

### 3.2 Session login (needed for everything except §7.1)

| Endpoint | Request | Response |
|---|---|---|
| `POST /api/v1/auth/login` | `{"email","password"}` | `200 {"user":{"id","email","display_name"},"session_id"}` + `Set-Cookie`. Errors: `401 invalid_credentials`, `429 authentication_throttled` (+ `Retry-After` seconds). |
| `GET /api/v1/auth/me` | cookie | `200 {"id","email","display_name"}`. Use as a cheap "is my session alive" check. |
| `POST /api/v1/auth/logout` | cookie | `204`. |
| `PATCH /api/v1/auth/me` | `{"display_name"}` | `200` same shape. |
| `GET /api/v1/auth/sessions` | cookie | list of the user's sessions. |
| `DELETE /api/v1/auth/sessions/{id}` | cookie | `204`. |
| `GET /api/v1/setup/status` | — | `200 {"complete": bool}` (first-run check). |
| `POST /api/v1/setup/complete` | `{"token","email","display_name","password","workspace_name","project_name"}` | First-run setup. `201 {"user_id","workspace_id","project_id","session_id"}`. |
| `POST /api/v1/auth/password`, `/auth/recovery/request`, `/auth/recovery/complete` | — | Password change / recovery (not needed by the bot; body of `/auth/password` **(unverified)**). |

Cookie details:

- Production: `__Host-orbit_session=…; Path=/; Secure; HttpOnly; SameSite=Lax`.
  Development: `orbit_session_dev=…` (no `Secure`).
- Lifetime: **30 days idle, 90 days absolute**. Each authenticated request
  refreshes the idle timer.
- Missing/expired cookie → `401 authentication_required`. **On 401, log in again
  and retry once.**
- Login is in the "authentication" rate-limit class (30/min/IP) and has extra
  per-email+IP throttling (15-minute window). Do not log in per request; keep the
  cookie (e.g. in memory and in the bot's data file).
- With `reqwest`: enable a cookie store (`reqwest::ClientBuilder::cookie_store(true)`)
  or copy the `Set-Cookie` value into a `Cookie` header yourself.

### 3.2.1 Bot account setup (chosen approach for orbit-bot)

The bot does **not** use a copied browser cookie. It stores the bot user's
email and password, calls `POST /api/v1/auth/login` at startup, and keeps the
cookie from the `Set-Cookie` header in memory. On `401 authentication_required`
it logs in again and retries once. This survives the 30-day idle / 90-day
absolute session limits with no manual work.

Create the bot user (one time, in the web UI):

1. Sign in to Orbit as the owner. Open **Settings → Members → Invite a member**.
2. Enter an email for the bot (it does not need to receive mail; delivery is
   `manual`), role **Member**, then copy the invitation link
   (`{origin}/accept-invitation?token=…`, valid 7 days).
3. Open the link in a **private/incognito window** (so you are not signed in as
   the owner). Fill in display name (e.g. `Orbit Bot`) and a password of
   **12–128 characters** that is not a common password.
4. Put the values in the bot's `.env`:
   ```
   ORBIT_URL=https://orbit.example.com     # public origin, no trailing slash
   ORBIT_EMAIL=orbit@example.com
   ORBIT_PASSWORD=…
   ORBIT_WORKSPACE_ID=                      # optional; else use the only workspace
   ```

Manual check with curl (the login POST also needs `Origin`):

```bash
curl -i -c jar.txt -X POST "$ORBIT_URL/api/v1/auth/login" \
  -H "Origin: $ORBIT_URL" -H 'Content-Type: application/json' \
  -d '{"email":"'"$ORBIT_EMAIL"'","password":"'"$ORBIT_PASSWORD"'"}'
# → 200, Set-Cookie: __Host-orbit_session=… (prod) or orbit_session_dev=… (dev)
curl -sS -b jar.txt "$ORBIT_URL/api/v1/auth/me"         # → {"id","email","display_name"}
curl -sS -b jar.txt "$ORBIT_URL/api/v1/workspaces"      # → [{"id", "name", "role":"member", …}]
```

In production, `__Host-` cookies are `Secure`: the client must talk to Orbit over
`https://` (the public URL), or the cookie is not stored or sent.

### 3.3 Permissions (cookie sessions)

Roles: `owner`, `admin`, `member` (no guest). **Any member can create, edit,
delete and restore tasks, comments, attachments, projects, statuses and labels.**
Owner/admin is needed for invitations, role changes, audit, API tokens, renaming
the workspace and deleting teamspaces. Owner only: delete/restore workspace,
transfer ownership. Not a member → `404` (never 403).

### 3.4 Origin / CSRF rule

Checked before routing (`OriginPolicy::permits`):

- `GET`/`HEAD`/`OPTIONS` (without `Upgrade`) → always allowed.
- Exempt paths (no Origin needed): `/api/v1/integrations/discord/events`,
  `/api/v1/integrations/github/webhook`, `/oauth/register|token|revoke`, and
  `/mcp` when no Origin is sent.
- **Every other POST/PUT/PATCH/DELETE, and every WebSocket upgrade, must carry
  exactly one `Origin` header equal to the public origin**, e.g.
  `Origin: https://orbit.example.com`. Otherwise `403 origin_forbidden`.

---

## 4. Conventions

1. **JSON is snake_case** everywhere (`workspace_id`, `status_id`, `due_at`).
   This is different from the old camelCase API.
2. **Unknown fields are rejected.** Bodies and query strings use
   `deny_unknown_fields` → `400 invalid_request`. Send only documented fields.
3. **IDs** are canonical **lowercase UUIDv7** strings. A malformed ID in a path,
   body or query returns **404**, not 400/422.
4. **The server creates IDs.** You never send `id` on create (unlike the old `"id": ""`).
5. **Timestamps** are RFC 3339 UTC with milliseconds: `"2026-10-01T12:00:00.000Z"`.
   Input may have at most millisecond precision.
6. **Everything is workspace-scoped.** Almost every path starts with
   `/api/v1/workspaces/{workspace_id}/…`. Get `workspace_id` from `GET /api/v1/workspaces`.
7. **Optimistic concurrency is mandatory on writes.** Records have an integer
   `version` (starts at 0; saved views start at 1). You send `expected_version`:
   - in the JSON body for PATCH, bulk/reorder items and `…/restore`;
   - as a **required query parameter** for DELETE: `DELETE …?expected_version=3`.
   - Mismatch → `409 conflict` with the current record (§5). No `If-Match`/ETag.
8. **Pagination:** query `cursor` + `limit` (default 50, clamped to 1..=100).
   Response `{"items": [...], "next_cursor": "<opaque>" | null}`. Send the same
   filters with the cursor, else `400 invalid_cursor`. No total count.
   Exceptions (not paginated): `GET /workspaces` and `GET /views` return a plain
   array; `GET /teamspaces` returns `{items}`; task relations return a plain array.
9. **Rate limits** (fixed 1-minute window per client IP, IPv6 per /64):

   | Class | Paths | Default per minute |
   |---|---|---|
   | Authentication | paths ending `/login`, containing `/session` or `/setup`, OAuth endpoints | 30 |
   | Recovery | containing `/recovery` | 10 |
   | Invitation | containing `/invitations` | 60 |
   | Upload | containing `/attachments` | 120 |
   | General | everything else (incl. Discord route, `/mcp`) | 600 |

   Over the limit → `429 rate_limit_exceeded`, **without** `Retry-After` (only
   login throttling sends `Retry-After`). Back off for ~60 s.
10. **Soft delete.** `DELETE` on tasks, projects, workspaces and pages moves the
    item to a trash for **30 days**; `POST …/restore` brings it back. Labels,
    statuses and comments are hard-deleted.
11. **Success codes:** `GET`/`PATCH` → 200; create → 201; `DELETE` → 204. Some
    actions differ (project restore → 200 with body; member role change → 204;
    notification read → 200 with body).
12. Every response has `X-Request-Id`. Log it with errors.

---

## 5. Error format

All errors (except OAuth endpoints) are `Content-Type: application/problem+json`:

```json
{
  "type": "https://docs.orbit.dev/problems/validation_failed",
  "title": "Validation failed",
  "status": 422,
  "code": "validation_failed",
  "detail": "title",
  "instance": "/api/v1/workspaces/0199…/tasks",
  "request_id": "0199…"
}
```

- Branch on HTTP status, then on `code`. `title`/`detail` are for logs.
- On task/project/status/label routes, `detail` of `validation_failed` is **just
  the field name** (`"title"`, `"status_id"`, `"label_ids"`, `"due_start_at"` …).
  On the Discord route it is a sentence. The `errors` map is never sent.
- `409 conflict` (version mismatch) adds:
  ```json
  "conflict": {
    "current_version": 7,
    "current": { "...full current record..." },
    "refresh": "/api/v1/workspaces/{ws}/tasks/{id}"
  }
  ```
  (Workspace/member conflicts have no `current`.) `409 restore_conflict` adds
  `"conflict": {"field": "key"}`. `422 invalid_filter` adds `"path": "filter.children[2].value"`.

### Codes the bot will meet

| HTTP | `code` | Meaning |
|---|---|---|
| 400 | `invalid_request` | Bad JSON, wrong `Content-Type`, unknown field/param, wrong type, bad date string, missing required field. |
| 400 | `invalid_cursor` | Cursor reused with a different query. |
| 400 | `https_required` | Production server reached without trusted HTTPS proxy. |
| 401 | `authentication_required` | No/expired session cookie → log in again. |
| 401 | `api_token_required` / `invalid_api_token` | Discord route: missing / bad token. |
| 403 | `origin_forbidden` | Cookie mutation without the correct `Origin`. |
| 403 | `api_token_scope_forbidden` | Token lacks `write` (Discord) or `read` (MCP). |
| 403 | `workspace_action_forbidden` | Role too low (admin actions). |
| 404 | `task_resource_not_found` | Task/project/status/label/view not found, trashed, malformed id, status not in project, or caller not a member. |
| 404 | `workspace_resource_not_found` | Workspace/member/invitation/token not found. |
| 404 | `attachment_not_found`, `route_not_found` | |
| 405 | `method_not_allowed` | |
| 409 | `conflict` | `expected_version` is stale. Use `conflict.current`. |
| 409 | `task_conflict` | Other state conflict: duplicate project key or label name, status still in use, relation conflict. |
| 409 | `github_content_read_only` | Title/description of a GitHub-linked task cannot be edited. |
| 409 | `integration_event_conflict` | Discord route: same `event_id`, different content. |
| 409 | `restore_conflict`, `contract_mismatch` | |
| 413 | `request_too_large`, `upload_too_large` | |
| 422 | `validation_failed` | Field rule broken (see `detail`). |
| 422 | `invalid_filter` | `/tasks/query` filter tree invalid. |
| 422 | `parent_cycle`, `parent_invalid` | Sub-issue parent problems. |
| 422 | `integration_project_unavailable` | Discord route: project gone / has no default status. |
| 429 | `rate_limit_exceeded`, `authentication_throttled` | |
| 500 | `internal_error` | Generic server failure. |

Other codes exist for pages, imports, GitHub, backups and admin; see §9.

---

## 6. Data model

### 6.1 Priority (enum)

Lowercase string: `"none" | "low" | "medium" | "high" | "urgent"`.
Default on create: `"none"`. (The old API had no `none` and defaulted to `medium`.)

### 6.2 Workspace

```json
{ "id": "0199…", "name": "Acme", "role": "owner", "version": 0, "deleted_at": null }
```

`role` is the **caller's** role. A new workspace gets a default project
**"General"** (key `GEN`, color `#5e6ad2`) with the default statuses, and a
default teamspace "General".

### 6.3 Member

```json
{ "id": "<membership id>", "user_id": "<user id>", "email": "alice@example.com",
  "display_name": "Alice", "role": "admin", "version": 0,
  "created_at": "2026-09-01T10:00:00.000Z", "suspended_at": null }
```

`id` is the **membership** id (used in `/members/{membership_id}`).
**Task `assignee_ids` use `user_id`**, not the membership id.

### 6.4 Project

```json
{ "id": "0199…", "workspace_id": "0199…", "name": "Product", "key": "PROD",
  "color": "#123456", "auto_close_parent": true, "auto_close_sub_issues": true,
  "version": 0, "deleted_at": null,
  "created_at": "2026-10-01T12:00:00.000Z", "updated_at": "2026-10-01T12:00:00.000Z" }
```

- `key`: trimmed, uppercased by the server, 1–20 chars of `A-Z 0-9 _`, unique per
  workspace among live projects.
- `color`: `#rrggbb`, stored lowercase.
- No description, icon, image, sort order or embedded statuses/labels (unlike
  the old API). Statuses come from their own endpoint; labels are workspace-level.
- `auto_close_parent` / `auto_close_sub_issues`: sub-issue automation (§11).

### 6.5 Status (per project)

```json
{ "id": "0199…", "workspace_id": "0199…", "project_id": "0199…",
  "name": "In Progress", "description": "", "color": "#f2c94c",
  "category": "started", "position": 2, "version": 0 }
```

- `category`: `unstarted | started | completed | cancelled | duplicate`
  (note **`cancelled`**, two Ls). There is no `backlog` category and no
  `isCompleted` flag: **"done" = category `completed`**. "Closed" = `completed`,
  `cancelled` or `duplicate`.
- `duplicate` is system-managed: it cannot be created, deleted, or set directly on a task.
- Ordered by `position` ascending, then `id`.

Default statuses of every new project:

| position | name | color | category |
|---|---|---|---|
| 0 | Backlog | `#8b8f98` | unstarted |
| 1 | Todo | `#8b8f98` | unstarted |
| 2 | In Progress | `#f2c94c` | started |
| 3 | Done | `#4cb782` | completed |
| 4 | Cancelled | `#8b8f98` | cancelled |
| 5 | Duplicate | `#8b8f98` | duplicate |

Users can rename, recolor, add and reorder statuses, so **never hard-code status
IDs or names**. Look them up per project and pick by `category`.

**"Default status"** (used by the Discord route; useful for the bot's `add` too):
the first non-`duplicate` status, `unstarted` category first, then lowest
`position`, then `id`. With defaults this is "Backlog".

### 6.6 Label (workspace-level)

```json
{ "id": "0199…", "workspace_id": "0199…", "name": "bug", "color": "#ff0000", "version": 0 }
```

- Labels belong to the **workspace**, not to a project. Any task in the workspace
  can use any label.
- Name 1–100 chars, unique per workspace (DB `UNIQUE(workspace_id, name)`; case
  sensitivity **(unverified)**, treat as case-sensitive and compare case-insensitively yourself).
- Tasks reference labels by **id** (`label_ids`), not by name.

### 6.7 Task

| Field | JSON type | Notes |
|---|---|---|
| `id` | string | UUIDv7, server-generated. |
| `workspace_id` | string | |
| `project_id` | string | |
| `status_id` | string | A status of `project_id`. |
| `title` | string | Trimmed, 1–500 chars. |
| `description` | string | Plain text rendered as **Markdown** in the UI. Default `""`. ≤ 100 000 chars. |
| `source_url` | string \| null | Absolute http(s) URL ≤ 2048 bytes. The Discord route stores the message URL here. |
| `priority` | string | §6.1. |
| `position` | integer | Manual sort rank within (project, status). |
| `creator_id` | string \| null | User id; `null` when created by a service account. |
| `creator_service_account_id` | string \| null | |
| `creator_service_account_name` | string \| null | |
| `assignee_ids` | string[] | **User** ids, sorted. Suspended/removed members are filtered out. |
| `label_ids` | string[] | Workspace label ids, sorted. |
| `due_start_at` | string \| null | Optional range start; requires `due_at`, must be ≤ `due_at`. |
| `due_at` | string \| null | **Full datetime**, not a date. |
| `version` | integer | +1 on every write. |
| `deleted_at` | string \| null | Set only in trash listings and conflict bodies. |
| `created_at`, `updated_at` | string | |
| `duplicate_of` | TaskRef \| null | When marked as duplicate. |
| `blocked` | boolean | A live, not-closed task `blocks` this one. |
| `parent_task_id` | string \| null | Sub-issue parent. |
| `parent` | TaskRef \| null | Parent, when live. |
| `sub_issue_count`, `sub_issue_closed_count` | integer | Direct children. |
| `ancestors` | TaskRef[] | **Only** on `GET /tasks/{id}`. |
| `trashed_descendant_count` | integer | **Only** in `GET /tasks/trash`. |

`TaskRef` = `{ "id", "project_id", "project_key", "title" }`.

**Fields that do NOT exist** (old API had them): `code`, `startAt` (use
`due_start_at`), `deadlineAt` (use `due_at`), `labels` as names, `subtasks`
(use sub-issues = child tasks via `parent_task_id`), `attachments` array,
`additionalInfo`, `archivedAt` (use soft delete), `completed`/`completed_at`.

**There is no human task code like `PM-12`.** The web UI shows a display code
built on the client: `` `${project.key}-${task.id.slice(-4).toUpperCase()}` ``
(e.g. `PROD-4A5B`). It is not stored, cannot be looked up through the API, and is
not unique (16 bits → collisions after a few hundred tasks per project).

Full example (`GET /tasks/{id}`):

```json
{
  "id": "0199a3f2-6b1c-7d24-9e0a-3c5b7d9e4a5b",
  "workspace_id": "0199a3e0-1a2b-7c3d-8e4f-5a6b7c8d9e0f",
  "project_id": "0199a3e0-1a2c-7f10-9a11-223344556677",
  "status_id": "0199a3e0-1a2d-7a01-8b02-c3d4e5f60718",
  "title": "Fix login redirect loop",
  "description": "Steps to reproduce:\n\n1. Log out\n2. Open `/settings`",
  "source_url": "https://discord.com/channels/111/222/333",
  "priority": "high",
  "position": 3,
  "creator_id": "0199a3e0-0f00-7aaa-8bbb-ccccdddd0001",
  "creator_service_account_id": null,
  "creator_service_account_name": null,
  "assignee_ids": ["0199a3e0-0f00-7aaa-8bbb-ccccdddd0001"],
  "label_ids": ["0199a3e5-5555-7666-8777-888899990000"],
  "due_start_at": null,
  "due_at": "2026-10-10T17:00:00.000Z",
  "version": 2,
  "deleted_at": null,
  "created_at": "2026-10-01T09:12:45.123Z",
  "updated_at": "2026-10-01T10:02:11.870Z",
  "duplicate_of": null,
  "blocked": false,
  "parent_task_id": null,
  "parent": null,
  "sub_issue_count": 0,
  "sub_issue_closed_count": 0,
  "ancestors": []
}
```

### 6.8 Comment

```json
{ "id": "…", "workspace_id": "…", "task_id": "…", "author_id": "…", "parent_id": null,
  "body": "Markdown text", "version": 0, "created_at": "…Z", "updated_at": "…Z" }
```

### 6.9 Attachment

```json
{ "id": "…", "workspace_id": "…", "task_id": "…", "comment_id": null, "owner_id": "…",
  "display_name": "screenshot.png", "media_type": "image/png", "byte_size": 48213,
  "created_at": "…Z" }
```

### 6.10 Notification

```json
{ "id": "…", "workspace_id": "…", "recipient_user_id": "…", "actor_user_id": "…",
  "kind": "task_assigned", "task_id": "…", "comment_id": null,
  "page_id": null, "page_thread_id": null, "page_comment_id": null, "page_block_id": null,
  "read_at": null, "created_at": "…Z" }
```

`kind`: `task_assigned`, `comment_mentioned`, `page_comment_mentioned`, `page_mentioned`.

### 6.11 Audit event (task activity, workspace audit)

```json
{ "id": "…", "workspace_id": "…", "actor_id": "…", "action": "task.updated",
  "outcome": "success", "resource_type": "task", "resource_id": "…",
  "request_id": "…", "metadata": {}, "occurred_at": "…Z" }
```

---

## 7. Endpoint reference — bot-critical

Paths in §7.3–§7.9 are relative to `/api/v1/workspaces/{workspace_id}` and need
the **session cookie** (+ `Origin` on mutations).

### 7.1 Discord integration (API token) — `POST /api/v1/integrations/discord/events`

The one endpoint built for Discord bots. It turns a Discord message into a task.

```
POST /api/v1/integrations/discord/events
Authorization: Bearer orb_…            (scope: write)
Content-Type: application/json
```

No `Origin` and no signature are needed.

```json
{
  "event_id": "discord-message-1290000000000000000",
  "message": "Login page crashes on Safari\nSteps: open /login …",
  "message_url": "https://discord.com/channels/111/222/333",
  "project_id": "0199a1b2-0000-7000-8000-000000000001"
}
```

| Field | Rules |
|---|---|
| `event_id` | Required, trimmed, 1–200 chars. Idempotency key, unique per workspace. A good choice: `discord-message-{message_id}`. |
| `message` | Required, must contain non-whitespace, ≤ 100 000 chars. Stored verbatim as `description`. |
| `message_url` | Required. Must start with `https://discord.com/channels/` (not `discordapp.com`, not `ptb.`), no whitespace, ≤ 2048 chars. Stored as `source_url`. |
| `project_id` | Optional. Must be one of the token's live projects. **Required if the token has more than one project.** |

What the server does (one transaction):

1. Title = first non-empty line of `message`, whitespace collapsed. If longer
   than 40 chars → first 37 chars + `"..."`. No usable line → `"Discord message"`.
   **You cannot choose the title, status, priority, labels, assignees or due date.**
2. Status = the project's default status (§6.5). Priority `none`. Position = end.
3. Adds the workspace label **`Discord`** (auto-created, color `#5865F2`).
4. Creator = the token's service account (or the token's user).
5. Dedup on `event_id`:
   - same `message` + `message_url` → **`200`** with `"duplicate": true` (existing task);
   - different content → **`409 integration_event_conflict`**;
   - mapped task was deleted → a **new** task is created (`201`).

Response `201` (new) or `200` (duplicate):

```json
{ "task": { /* Task (§6.7) without ancestors / trashed_descendant_count */ }, "duplicate": false }
```

Errors:

| Status | code | When |
|---|---|---|
| 400 | `invalid_request` | Bad JSON / unknown field / missing field. |
| 401 | `api_token_required` | No or malformed `Authorization: Bearer`. |
| 401 | `invalid_api_token` | Token unknown, revoked, expired, or otherwise invalid (§3.1). |
| 403 | `api_token_scope_forbidden` | Token lacks `write`. |
| 409 | `integration_event_conflict` | Same `event_id`, different message/URL. Do not resend edited messages with the same id. |
| 422 | `validation_failed` | `detail` is one of: "Select a project that the API token can write to." / "Event IDs must contain between 1 and 200 characters." / "Message URLs must be Discord channel message URLs of at most 2048 characters." / "Messages must contain text and the stored description must not exceed 100000 characters." |
| 422 | `integration_project_unavailable` | Project gone or has no usable status. |

To change the task after creation (title, status, priority…), you need a cookie
session and `PATCH /tasks/{id}` (§7.6).

### 7.2 Workspaces (discovery)

| Method | Path | Success | Returns |
|---|---|---|---|
| GET | `/api/v1/workspaces` | 200 | `Workspace[]` (plain array, not paginated) |
| GET | `/api/v1/workspaces/{workspace_id}` | 200 | `Workspace` |
| GET | `/api/v1/workspaces/{workspace_id}/members?cursor&limit` | 200 | `Page<Member>` |

Use these at startup to resolve `workspace_id` and to map users to `user_id`
(for assignees and display names).

### 7.3 Projects

| Method | Path | Body | Success | Returns |
|---|---|---|---|---|
| GET | `/projects?cursor&limit` | — | 200 | `Page<Project>` (live only, ordered by `name, id`) |
| POST | `/projects` | `{"name","key","color"}` | 201 | `Project` (+ 6 default statuses created) |
| PATCH | `/projects/{project_id}` | `{"name","key","color","expected_version","auto_close_parent"?,"auto_close_sub_issues"?}` | 200 | `Project` |
| DELETE | `/projects/{project_id}?expected_version=N` | — | 204 | soft delete (tasks hidden) |
| GET | `/projects/trash?cursor&limit` | — | 200 | `Page<Project>` |
| POST | `/projects/{project_id}/restore` | `{"expected_version"}` | 200 | `Project` |

- There is **no `GET /projects/{id}`**: list and filter.
- PATCH is a **full replacement** of `name`, `key`, `color` (all required).
- Errors: `422 validation_failed` (`name`/`key`/`color`), `409 task_conflict`
  (duplicate key), `409 conflict`, `409 restore_conflict {"field":"key"}`.

### 7.4 Statuses (per project)

| Method | Path | Body | Success | Returns |
|---|---|---|---|---|
| GET | `/projects/{project_id}/statuses?cursor&limit` | — | 200 | `Page<Status>` ordered by `position, id` |
| POST | `/projects/{project_id}/statuses` | `{"name","color","category","description"?,"position"?}` | 201 | `Status` |
| PATCH | `/projects/{project_id}/statuses/{status_id}` | `{"name","color","category","position","expected_version","description"?}` | 200 | `Status` |
| DELETE | `/projects/{project_id}/statuses/{status_id}?expected_version=N` | — | 204 | hard delete |
| POST | `/projects/{project_id}/statuses/reorder` | `{"items":[{"id","expected_version","position"}]}` (1–100) | 200 | `Page<Status>` (changed ones) |

- `category` on create: `unstarted|started|completed|cancelled` (`duplicate` → 422).
- PATCH is a full replacement (all listed fields required); `description` optional.
- **Delete has no "move tasks to" option**: if any task (even trashed) uses the
  status → `409 task_conflict`. Move tasks first.

### 7.5 Labels (workspace-level)

| Method | Path | Body | Success | Returns |
|---|---|---|---|---|
| GET | `/labels?cursor&limit` | — | 200 | `Page<Label>` ordered by `name, id` |
| POST | `/labels` | `{"name","color"}` | 201 | `Label` |
| PATCH | `/labels/{label_id}` | `{"name","color","expected_version"}` (all required) | 200 | `Label` |
| DELETE | `/labels/{label_id}?expected_version=N` | — | 204 | hard delete; removed from all tasks |

Duplicate name → `409 task_conflict`. To tag a task with a new label: create the
label, then PATCH the task's `label_ids` (no project workflow step, unlike the old API).

### 7.6 Tasks — CRUD

| Method | Path | Body / query | Success | Returns |
|---|---|---|---|---|
| GET | `/tasks` | query (below) | 200 | `Page<Task>` |
| POST | `/tasks` | `TaskCreate` | **201** | `Task` + `auto_closed` |
| GET | `/tasks/{task_id}` | — | 200 | `Task` (with `ancestors`) |
| PATCH | `/tasks/{task_id}` | `TaskPatch` | 200 | `Task` + `auto_closed` |
| DELETE | `/tasks/{task_id}?expected_version=N` | — | **204** | soft delete (also trashes sub-issues) |
| GET | `/tasks/trash?cursor&limit` | — | 200 | `Page<Task>` (deleted ≤ 30 days, newest first) |
| POST | `/tasks/{task_id}/restore` | `{"expected_version"}` (the trashed version) | 200 | `Task` |

`task_id` must be the UUID. A display code like `PROD-4A5B` → 404.

#### `GET /tasks` query parameters

Lists live tasks (task and project not trashed). **Completed tasks are included.**
Sub-issues are included. All conditions are ANDed. Unknown params → 400.

| Param | Type | Default | Meaning |
|---|---|---|---|
| `project_id` | UUID | — | Only this project. |
| `status_id` | UUID | — | Exact status. |
| `assignee_id` | UUID (user) | — | Assigned to this user. Ignored with `view=mine`/`my_week`. |
| `unassigned` | bool | `false` | Only tasks without assignees. |
| `label_id` | UUID | — | Has this label. |
| `priority` | enum | — | §6.1. |
| `search` | string ≤ 200 | — | Case-insensitive (ASCII) **substring of title OR description**. |
| `view` | enum | — | `mine`, `overdue`, `due_soon`, `current_week`, `my_week` (§7.7 presets). |
| `parent_task_id` | UUID | — | Only direct children. |
| `sort` | enum | `position` | `position`, `priority`, `title`, `created_at`, `updated_at`, `due_date`. |
| `order` | enum | `asc` | `asc`, `desc`. |
| `cursor`, `limit` | | 50 | §4.8 |

`sort=priority&order=asc` puts **urgent first**. `due_date` sort puts undated
tasks last in both directions. To hide done tasks, use `POST /tasks/query` with
`show_completed: "none"` (there is no such flag on `GET /tasks`).

#### `POST /tasks` body (`TaskCreate`)

| Field | Type | Required | Default | Rules |
|---|---|---|---|---|
| `project_id` | UUID | **yes** | — | Live project in the workspace, else 404. |
| `status_id` | UUID | **yes** | — | **No server default.** Must belong to `project_id` (else 404). `duplicate` category → 422. |
| `title` | string | **yes** | — | Trimmed, 1–500 chars. |
| `description` | string | no | `""` | ≤ 100 000 chars. |
| `source_url` | string \| null | no | null | http(s) URL ≤ 2048 bytes. |
| `priority` | enum | no | `"none"` | |
| `position` | integer | no | end of (project, status) | |
| `assignee_ids` | UUID[] | no | `[]` | User ids of non-suspended members; no duplicates. |
| `label_ids` | UUID[] | no | `[]` | Workspace labels; no duplicates. |
| `due_start_at` | RFC 3339 \| null | no | null | Needs `due_at`; ≤ `due_at`. |
| `due_at` | RFC 3339 \| null | no | null | |
| `parent_task_id` | UUID \| null | no | null | Live task in the workspace → sub-issue. |

Response `201`: the Task fields **flattened** plus `"auto_closed": [{ "id", "status_id" }]`.
Side effects: `task_assigned` notifications to assignees (except the caller), audit `task.created`.

#### `PATCH /tasks/{task_id}` body (`TaskPatch`)

Partial update. **`expected_version` is required.** Absent field = unchanged.

| Field | `null` allowed | Meaning |
|---|---|---|
| `expected_version` | no | Must equal the current `version`. |
| `project_id` | no | **Move** to another project. Send a `status_id` of the target project too. |
| `status_id` | no | Change status. Not `duplicate`. Cannot combine with `duplicate_of_id`. |
| `title`, `description` | no | GitHub-linked task → `409 github_content_read_only`. |
| `source_url` | **yes** | `null` clears. |
| `priority` | no | |
| `position` | no | |
| `assignee_ids` | no | **Replaces** the set; `[]` unassigns all. |
| `label_ids` | no | **Replaces** the set; `[]` clears. |
| `due_start_at` | **yes** | `null` clears. |
| `due_at` | **yes** | `null` clears (and also clears `due_start_at` if not sent). |
| `duplicate_of_id` | **yes** | Mark/unmark as duplicate of a live task. |
| `parent_task_id` | **yes** | Set/clear sub-issue parent (`parent_cycle`/`parent_invalid` on 422). |

`null` on any other field → `400 invalid_request`.

Response `200`: Task (flattened, no `ancestors`) + `auto_closed`.

### 7.7 Tasks — query DSL: `POST /tasks/query`

```json
{
  "filter": {"op": "and", "children": [
    {"field": "project", "operator": "is", "value": ["<project id>"]},
    {"field": "status_category", "operator": "is_not", "value": ["completed", "cancelled", "duplicate"]},
    {"op": "or", "children": [
      {"field": "text", "operator": "contains", "value": "login"},
      {"field": "label", "operator": "includes_any", "value": ["<label id>"]}
    ]}
  ]},
  "order_by": "priority",
  "order_direction": "asc",
  "show_completed": "none",
  "limit": 25
}
```

| Field | Required | Values |
|---|---|---|
| `filter` | yes | FilterGroup. `{"op":"and","children":[]}` matches all. |
| `order_by` | yes | `manual` (= position asc), `priority`, `created`, `updated`, `title`, `due_date` |
| `order_direction` | yes | `asc`, `desc` |
| `show_completed` | yes | `all`, `past_week`, `past_month`, `none` (hides completed/cancelled/duplicate) |
| `cursor`, `limit` | no | |
| `parent_task_id` | no | Only direct children. |
| `sub_issues` | no | `nested` (default), `flat`, `hidden` (top-level only) |

Grammar:

```
FilterGroup := { "op": "and" | "or", "children": [FilterNode, ...] }
FilterNode  := FilterGroup | Condition
Condition   := { "field": Field, "operator": Operator, "value"?: Value }
```

Limits: depth ≤ 3 below root, ≤ 50 conditions, ≤ 100 values per array, text 1–200 chars.

| `field` | Operators | `value` |
|---|---|---|
| `status` | `is`, `is_not` | Status UUIDs, or `"<category>:<name>"` (e.g. `"completed:done"`, case-insensitive, works across projects) |
| `status_category` | `is`, `is_not` | `unstarted`, `started`, `completed`, `cancelled`, `duplicate` |
| `assignee` | `is`, `is_not`, `is_empty`, `is_not_empty` | User UUIDs or `"me"` |
| `creator` | `is`, `is_not` | User UUIDs or `"me"` |
| `label` | `includes_any`, `includes_all`, `excludes`, `is_empty`, `is_not_empty` | Label UUIDs |
| `priority` | `is`, `is_not` | Priority strings |
| `project` | `is`, `is_not` | Project UUIDs |
| `due_date` | `before`, `after`, `between`, `is_empty`, `is_not_empty` | DateValue, or `[DateValue, DateValue]` for `between` |
| `created_at`, `updated_at` | `before`, `after` | DateValue |
| `text` | `contains` | String (title or description) |
| `parent` | `is`, `is_not` | Task UUIDs and/or `"none"` |
| `sub_issues` | `is` | `"has"` / `"none"` |

`is_empty`/`is_not_empty` take no `value`.
`DateValue` = `{"absolute": "YYYY-MM-DD"}` or
`{"relative": "today" | "start_of_week" | "end_of_week", "offset_days"?: -3650..3650}`.
Days are UTC; weeks start Monday. `before D` = `< start of D`; `after D` = `≥ start of D+1`;
`between [a,b]` = `≥ start of a` and `< start of b+1`.

Presets used by `GET /tasks?view=` ("not done" = `status_category is_not [completed, cancelled, duplicate]`):

| view | conditions |
|---|---|
| `mine` | `assignee is ["me"]` |
| `overdue` | `due_date before today` + not done |
| `due_soon` | `due_date between [today, today+6]` + not done |
| `current_week` | `due_date between [start_of_week, end_of_week]` + not done |
| `my_week` | `current_week` + `mine` |

Errors: `422 invalid_filter` with `path`, `400 invalid_cursor`.

### 7.8 Tasks — bulk and reorder

| Method | Path | Body | Success |
|---|---|---|---|
| POST | `/tasks/bulk` | `{"updates":[{"id","expected_version", …any TaskPatch field}]}` (1–100, unique ids) | 200 `{"items":[Task…],"next_cursor":null,"auto_closed":[…]}` |
| POST | `/tasks/reorder` | `{"items":[{"id","expected_version","position"}]}` (1–100) | 200 `Page<Task>` |

Both are **atomic** (all or nothing). There is no bulk delete.

### 7.9 Tasks — comments, activity, relations, attachments

**Comments** (body = Markdown text):

| Method | Path | Body | Success |
|---|---|---|---|
| GET | `/tasks/{task_id}/comments?cursor&limit` | — | 200 `Page<Comment>` (oldest first) |
| POST | `/tasks/{task_id}/comments` | `{"body", "parent_id"?, "mentioned_user_ids"?}` | 201 `Comment` |
| PATCH | `/tasks/{task_id}/comments/{comment_id}` | `{"body","expected_version"}` | 200 `Comment` |
| DELETE | `/tasks/{task_id}/comments/{comment_id}?expected_version=N` | — | 204 (hard delete) |

`body`: non-blank, ≤ 100 000 chars. `mentioned_user_ids` (user ids) create
`comment_mentioned` notifications. Any member can edit/delete any comment.

**Activity:** `GET /tasks/{task_id}/activity?cursor&limit` → `Page<AuditEvent>`.
Actions include `task.created`, `task.updated`, `task.deleted`, `task.restored`,
`task.parent_changed`, `task.marked_duplicate`, `task.relation_added`, `task.relation_removed`.

**Relations:**

| Method | Path | Body | Success |
|---|---|---|---|
| GET | `/tasks/{task_id}/relations` | — | 200 `[{ "id", "type": "blocks"\|"related"\|"duplicate", "direction": "outgoing"\|"incoming", "task": {"id","project_id","title","status_id"}, "created_at" }]` |
| POST | `/tasks/{task_id}/relations` | `{"type": "blocks"\|"blocked_by"\|"related", "task_id"}` | 201 relation |
| DELETE | `/tasks/{task_id}/relations/{relation_id}` | — | 204 |

**GitHub links:** `GET /tasks/{task_id}/github-links` →
`[{ "kind": "issue"|"pull_request", "title", "url", "state", "source" }]`.

**Attachments** (upload rate class 120/min):

| Method | Path | Success |
|---|---|---|
| GET | `/tasks/{task_id}/attachments?cursor&limit` | 200 `Page<Attachment>` (task-level only) |
| POST | `/tasks/{task_id}/attachments` | 201 `Attachment` |
| DELETE | `/tasks/{task_id}/attachments/{attachment_id}` | 204 |
| GET | `/tasks/{task_id}/attachments/{attachment_id}/download` | 200 bytes (`inline` for images, else `attachment`) |
| GET/POST | `/tasks/{task_id}/comments/{comment_id}/attachments` | list / upload to a comment |
| DELETE | `/tasks/{task_id}/comments/{comment_id}/attachments/{attachment_id}` | 204 |
| GET | `/tasks/{task_id}/comments/{comment_id}/attachments/{attachment_id}/download` | bytes |
| POST | `/tasks/{task_id}/comments/attachments` | 201 `{"comment": Comment, "attachments": [Attachment]}` (new empty-body comment + file) |

Upload = `multipart/form-data` with **exactly one** field named **`file`**
(a second file → `400 invalid_multipart`). Max 25 MiB per file by default
(`413 upload_too_large`). The media type is detected from the bytes, not taken
from the client. File name ≤ 255 bytes, no control characters.

### 7.10 Notifications (the session user's own)

| Method | Path | Success |
|---|---|---|
| GET | `/notifications?unread=true&cursor&limit` | 200 `Page<Notification>` (newest first) |
| POST | `/notifications/{notification_id}/read` | 200 `Notification` |
| POST | `/notifications/read-all` | 200 `{"updated": 3}` |

### 7.11 Realtime — `GET /api/v1/workspaces/{workspace_id}/events` (WebSocket)

- WebSocket (not SSE). Cookie session + `Origin` header on the handshake.
- Query `?after=<sequence>`. Without `after` you immediately get `resync_required`.
- Frames are **invalidation signals only** (no data):
  ```json
  {"version":1,"kind":"workspace.changed","sequence":"1234","workspaces_changed":false,"profile_changed":false,"views_only":false}
  ```
  `kind`: `workspace.changed` or `resync_required`. Re-fetch over HTTP, then
  reconnect with the last `sequence` as `after`.
- Server polls every 1 s, pings every 20 s, closes if no pong within 60 s.
  **It closes the socket if the client sends any message.**

Useful if the bot wants to refresh its cache (e.g. for autocomplete) when tasks change.

---

## 8. Endpoint reference — workspace administration

Cookie session. Paths under `/api/v1`.

| Method | Path | Body | Success | Role |
|---|---|---|---|---|
| POST | `/workspaces` | `{"name"}` (1–200) | 201 `Workspace` | any user |
| GET | `/workspaces/trash` | — | 200 `Workspace[]` | owner |
| PATCH | `/workspaces/{id}` | `{"name","expected_version"}` | 200 `Workspace` | owner/admin |
| DELETE | `/workspaces/{id}?expected_version=N` | — | 204 | owner |
| POST | `/workspaces/{id}/restore` | `{"expected_version"}` | 204 | owner |
| PATCH | `/workspaces/{id}/members/{membership_id}` | `{"role":"admin"\|"member","expected_version"}` | 204 | owner/admin |
| DELETE | `/workspaces/{id}/members/{membership_id}?expected_version=N` | — | 204 | owner/admin, or self (leave) |
| POST | `/workspaces/{id}/transfer-ownership` | `{"membership_id","expected_version","membership_version"}` | 204 | owner |
| GET | `/workspaces/{id}/invitations?cursor&limit` | — | 200 `Page<Invitation>` | owner/admin |
| POST | `/workspaces/{id}/invitations` | `{"email","role":"admin"\|"member","delivery":"manual"\|"smtp"}` | 201 `{"invitation", "url"?}` | owner/admin |
| DELETE | `/workspaces/{id}/invitations/{invitation_id}` | — | 204 | owner/admin |
| POST | `/workspaces/invitations/preview` | `{"token"}` | 200 `{"email","workspace_name"}` | public |
| POST | `/workspaces/invitations/accept` | `{"token"}` (signed in) or `{"token","email","display_name","password"}` (register) | 200 / 201 `{"workspace_id","membership_id","created","email_verified"}` | — |
| GET | `/workspaces/{id}/audit?cursor&limit` | — | 200 `Page<AuditEvent>` | owner/admin |
| GET/POST/DELETE | `/workspaces/{id}/api-tokens[/{token_id}]` | §3.1 | | owner/admin |

Invitation `status`: `pending | accepted | revoked | replaced | expired` (7-day expiry).

**Installation admin** (`403 installation_admin_required` otherwise):
`POST /admin/users/{user_id}/suspension {"suspended": bool}` → 204;
`GET /admin/audit?workspace_id&action&cursor&limit`; `GET /admin/audit/export` (CSV);
`GET|POST /admin/backups`; `GET /admin/backups/{backup_id}/download` (zip).
Backup `created_at` is a Unix-millisecond integer.

---

## 9. Endpoint reference — secondary areas

All cookie-only. The bot probably does not need these; they are listed for completeness.
Paths relative to `/api/v1/workspaces/{workspace_id}`.

### 9.1 Saved views and preferences

| Method | Path | Notes |
|---|---|---|
| GET / POST | `/views` | Plain array / create `{"name","description"?,"icon"?,"color"?,"visibility":"personal"\|"workspace","state":{filter,display}}` → 201 (version 1) |
| GET / PATCH / DELETE | `/views/{view_id}` | PATCH `{"expected_version", …optional}`; DELETE has no version |
| PUT / DELETE | `/views/{view_id}/favorite` | 204 |
| PUT | `/view-favorites/order` | `{"view_ids":[…]}` exact permutation → 204 |
| GET / PUT | `/view-preferences/{page_key}` | `page_key`: `all`, `project:<id>`, `preset:<mine\|overdue\|due_soon\|current_week\|my_week>`; PUT `{"state"}` |

`state.filter` uses the §7.7 grammar. `state.display` keys: `layout` (`list|board|timeline`),
`group_by`, `sub_group_by` (`status|assignee|priority|project|label|none`), `order_by`,
`order_direction`, `properties[]`, `show_completed`, `show_empty_groups`, `sub_issues`?.

### 9.2 Teamspaces (document spaces, unrelated to projects)

`GET /teamspaces` → `{items}`; `POST /teamspaces {"name","icon"?}`;
`PATCH /teamspaces/{id} {"expected_version","name"?,"icon"?}`;
`DELETE /teamspaces/{id}?expected_version=N` (owner/admin; `409 teamspace_not_empty`, `422 last_teamspace`);
`POST /teamspaces/{id}/move {"expected_version","position"}`.

### 9.3 Pages (documents)

Content is a **BlockNote JSON block array**; no Markdown input exists.

| Method | Path | Notes |
|---|---|---|
| GET / POST | `/pages` | Tree of `PageSummary` / create `{parent_id?, teamspace_id?, private?, title?, icon?, position?}` → 201 |
| GET / PATCH / DELETE | `/pages/{page_id}` | PATCH `{expected_version, title?, icon?, cover_url?, cover_position?, content?, full_width?}` (whole `content`); `423 page_locked` |
| POST | `/pages/{id}/move`, `/restore`, `/duplicate`, `/lock`, `/visit` | |
| DELETE | `/pages/{id}/permanent?expected_version=N` | Must be trashed first |
| GET / POST | `/pages/trash`, `/pages/trash/empty` | |
| GET | `/pages/search?q=`, `/pages/recent?limit=`, `/pages/favorites` | |
| PUT / DELETE | `/pages/{id}/favorite`; POST `/pages/favorites/{id}/move` | |
| GET / POST | `/pages/{id}/versions`, `/versions/{version_id}`, `/versions/{version_id}/restore` | History |
| POST / GET | `/pages/{id}/files`, `/pages/{id}/files/{file_id}` | Multipart `file` |
| GET / POST | `/pages/{id}/threads`; `/threads/{thread_id}/comments`, `/resolve`, `/reopen`; PATCH/DELETE comments | Comment bodies are BlockNote paragraph blocks |
| GET | `/pages/{id}/export?format=markdown&children=` | Markdown **inside a ZIP** |
| GET | `/pages/{id}/collab?v=1&epoch=` | Yjs y-sync WebSocket (not practical for a bot) |

### 9.4 Notion import

`POST /imports/notion {"token"}` → `GET /imports/notion[/{id}?include=tree]` →
`POST /imports/notion/{id}/start {"selection":{"all":true}|{"notion_ids":[…]},"destination"?}` →
`POST /imports/notion/{id}/cancel`.

### 9.5 GitHub integration

- `POST /api/v1/integrations/github/webhook` (GitHub → Orbit, HMAC `X-Hub-Signature-256`).
- `GET /api/v1/integrations/github/manifest/callback`.
- Cookie: `GET /workspaces/{ws}/github`, `POST /workspaces/{ws}/github/manifest`,
  `GET|PUT|DELETE /workspaces/{ws}/projects/{project_id}/github`
  (PUT `{"installation_id","repository":"owner/repo","label"}`).
- GitHub-linked tasks: title/description are read-only.

### 9.6 MCP (read-only, API token with `read` scope or OAuth)

`POST /mcp`, JSON-RPC 2.0, Streamable HTTP. Headers:
`Authorization: Bearer orb_…`, `Content-Type: application/json`,
`Accept: application/json, text/event-stream`, `MCP-Protocol-Version: 2025-11-25`.

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tasks","arguments":{"project_id":"…","query":"login","limit":25}}}
```

| Tool | Arguments | Result |
|---|---|---|
| `list_projects` | — | Token's live projects |
| `list_tasks` | `project_id?`, `query?` (title/description), `cursor?`, `limit?` (1–100, default 50) | Tasks + `next_cursor`, newest first, completed included |
| `get_task` | `task_id` | One task |

Task projection: `id, project_id, status_id, title, description, priority,
assignee_ids, label_ids, due_start_at, due_at, created_at, updated_at, version`.
No statuses list and no mutations — so MCP alone cannot show status names.
OAuth discovery: `/.well-known/oauth-authorization-server`; grants are managed at
`GET /api/v1/oauth/grants`, `DELETE /api/v1/oauth/grants/{grant_id}`.

### 9.7 Health and metrics

- `GET /health/live` → `200 {"live": true}`.
- `GET /health/ready` → `200`/`503` `{"ready": bool, "checks": {…}}`.
- `/metrics` (Prometheus) is only on a separate private listener (`metrics.listen`).

---

## 10. Bot recipes

Assume: `BASE=https://orbit.example.com`, `ORIGIN=$BASE`, `WS=<workspace id>`,
session cookie in `$COOKIE` (e.g. `__Host-orbit_session=…`).

### 10.1 Startup

1. `POST /api/v1/auth/login` → keep the cookie.
2. `GET /api/v1/workspaces` → choose `workspace_id` (config value, or the only one).
3. Cache per workspace: `GET /projects` (all pages), and for each project
   `GET /projects/{id}/statuses`; `GET /labels`; `GET /members`.
4. Optional: open the realtime WebSocket (§7.11) and refresh the cache on
   `workspace.changed`, or simply refresh on a timer / on 404 / on 409.

### 10.2 Command mapping

| Bot command | Orbit calls |
|---|---|
| **add** (from slash command) | Pick project → status = default status (§6.5) or the user's choice → `POST /tasks` `{project_id, status_id, title, description?, priority?, label_ids?, due_at?}`. Store `id`. |
| **add** (from a Discord message, "Apps → Create task") | `POST /api/v1/integrations/discord/events` with a `write` token (§7.1). No cookie needed. |
| **list** | `POST /tasks/query` with `project` filter, `show_completed:"none"`, `order_by:"priority"`. Or `GET /tasks?project_id=…&sort=priority`. Map `status_id` → status name from cache. |
| **done** | Find the first status with `category == "completed"` in the task's project → `PATCH /tasks/{id}` `{"expected_version": v, "status_id": done_id}`. |
| **status** | `PATCH /tasks/{id}` `{"expected_version": v, "status_id": …}` (never a `duplicate` status). For a cross-project choice, use the status list of the task's project. |
| **delete** | `DELETE /tasks/{id}?expected_version=v` (soft; restorable for 30 days with `POST /tasks/{id}/restore`). |
| **autocomplete** | `GET /tasks?search=<typed>&limit=25&sort=updated_at&order=desc` (+ `project_id`). Use the task **UUID** as the choice value (Discord limit: 100 chars — a UUID fits). Show `PROJ-XXXX · title` as the label. |

### 10.3 Version handling

Always use the latest `version` you have. On `409 conflict`:
1. Read `conflict.current` (the full current task).
2. If your change still makes sense (e.g. "mark done"), retry once with
   `expected_version = conflict.current_version`.
3. Else tell the user that the task changed.

If you only have the task id, `GET /tasks/{id}` first to get `version`.

### 10.4 curl examples

```bash
# Log in (dev: cookie name is orbit_session_dev)
curl -sS -c jar.txt -X POST "$BASE/api/v1/auth/login" \
  -H "Origin: $ORIGIN" -H 'Content-Type: application/json' \
  -d '{"email":"orbit@example.com","password":"…"}'

# Workspaces
curl -sS -b jar.txt "$BASE/api/v1/workspaces"

# Statuses of a project
curl -sS -b jar.txt "$BASE/api/v1/workspaces/$WS/projects/$PROJECT/statuses"

# Create a task
curl -sS -b jar.txt -X POST "$BASE/api/v1/workspaces/$WS/tasks" \
  -H "Origin: $ORIGIN" -H 'Content-Type: application/json' \
  -d '{"project_id":"'"$PROJECT"'","status_id":"'"$TODO"'","title":"Fix login redirect loop","priority":"high","due_at":"2026-10-10T17:00:00.000Z"}'

# Open tasks, urgent first
curl -sS -b jar.txt -X POST "$BASE/api/v1/workspaces/$WS/tasks/query" \
  -H "Origin: $ORIGIN" -H 'Content-Type: application/json' \
  -d '{"filter":{"op":"and","children":[{"field":"project","operator":"is","value":["'"$PROJECT"'"]}]},"order_by":"priority","order_direction":"asc","show_completed":"none","limit":25}'

# Mark done
curl -sS -b jar.txt -X PATCH "$BASE/api/v1/workspaces/$WS/tasks/$TASK" \
  -H "Origin: $ORIGIN" -H 'Content-Type: application/json' \
  -d '{"expected_version":2,"status_id":"'"$DONE"'"}'

# Delete (soft)
curl -sS -b jar.txt -X DELETE "$BASE/api/v1/workspaces/$WS/tasks/$TASK?expected_version=3" \
  -H "Origin: $ORIGIN"

# Discord message → task (API token, no cookie, no Origin)
curl -sS -X POST "$BASE/api/v1/integrations/discord/events" \
  -H "Authorization: Bearer $ORBIT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"event_id":"discord-message-123","message":"Please fix this","message_url":"https://discord.com/channels/1/2/123","project_id":"'"$PROJECT"'"}'
```

### 10.5 Rust (reqwest) sketch

```rust
use reqwest::header::{HeaderMap, HeaderValue, ORIGIN};

let mut headers = HeaderMap::new();
headers.insert(ORIGIN, HeaderValue::from_str(&public_origin)?);
let http = reqwest::Client::builder()
    .cookie_store(true)              // needs reqwest feature "cookies"
    .default_headers(headers)        // Origin on every request is harmless for GETs
    .build()?;

http.post(format!("{base}/api/v1/auth/login"))
    .json(&serde_json::json!({ "email": email, "password": password }))
    .send().await?.error_for_status()?;

#[derive(serde::Deserialize)]
struct Page<T> { items: Vec<T>, next_cursor: Option<String> }

#[derive(serde::Deserialize)]
struct Problem { status: u16, code: String, detail: String }
```

Model structs with `#[serde(default)]` for optional fields; do **not** use
`deny_unknown_fields` on response types (Orbit may add fields). Request structs
must contain **only** documented fields (Orbit rejects unknown ones); use
`#[serde(skip_serializing_if = "Option::is_none")]` for optional PATCH fields and a
double `Option<Option<T>>` for the nullable ones (`due_at`, `due_start_at`,
`source_url`, `parent_task_id`, `duplicate_of_id`).

### 10.6 Production pattern for §7.1

- Env: `ORBIT_API_URL` (origin), `ORBIT_API_TOKEN` (write), `ORBIT_PROJECT_ID`
  **or** `ORBIT_PROJECTS` (`{"Name":"project-uuid", …}`; not both).
- `event_id = "discord-message-{message_id}"`,
  `message_url = "https://discord.com/channels/{guild}/{channel}/{message}"`.
- `201` → created, `200` → duplicate, `422` → project rejected (token not scoped
  to it), other → error.
- Task link: `{ORBIT_API_URL}/tasks/{task.id}?workspace={task.workspace_id}`.

---

## 11. Behaviour notes & edge cases

- **No task codes.** Display codes are client-side only (`KEY-XXXX` from the last
  4 hex chars of the UUID). Keep UUIDs as identifiers in the bot.
- **"Done" is a status category.** There is no complete endpoint and no
  `completed` flag in responses. Check `category` of the task's status.
- **Sub-issue automation.** Create/PATCH/bulk may change *other* tasks; they are
  listed in `auto_closed: [{id, status_id}]`.
  - `auto_close_parent` (default on): when all live children of a parent are
    closed and at least one is `completed`, the parent moves to its first
    `completed` status (recursively).
  - `auto_close_sub_issues` (default on): when a task becomes `completed`/
    `cancelled`, its open descendants move to their project's first status of
    the same category.
- **Delete cascades to sub-issues** (all trashed). Restore brings back
  descendants trashed at the same moment. Trash is purged after 30 days.
- **Deleting a project hides its tasks.** Restoring the project brings them back.
- **Moving a task between projects:** `PATCH {project_id, status_id, expected_version}`
  with a status of the target project. Labels are workspace-level, so they stay valid.
- **Dates are datetimes.** For a "date only" input, send e.g. `T23:59:59.000Z`
  or noon in the user's timezone converted to UTC. `due_start_at` requires `due_at`.
- **Search** is a substring match (`LIKE`) on title or description, ASCII
  case-folding only, max 200 chars. Not fuzzy.
- **Anyone in the workspace can edit anything** (tasks, comments, attachments).
  The bot user's actions show up in activity under the bot user's name.
- **Validation `detail` is a field name** on task routes; build user messages yourself.
- **Malformed IDs → 404.** Validate UUID format in the bot to give better errors.
- **Concurrency is strict.** Every write needs the latest `version`; even
  automation and reorder bump it.
- **Rate limit 429 has no `Retry-After`.** Wait until the next minute window.
- **Behind a reverse proxy**, all bot requests come from one IP; the 600/min
  general limit is per IP.
- **Production HTTPS rule.** Call Orbit through its public HTTPS URL, not the
  container port, or you get `400 https_required`.

---

## 12. Migration map: old PM API → Orbit

| Old PM API | Orbit |
|---|---|
| No auth | API token (Discord create only) **or** session cookie (everything else) + `Origin` |
| camelCase | **snake_case** |
| `"id": ""` on create | Do not send `id` |
| `PUT` full replace | `PATCH` partial (tasks) / full (projects, statuses, labels) + `expected_version` |
| `GET /api/bootstrap` (all data) | No single dump. `GET /workspaces` → `/projects` → `/projects/{id}/statuses` → `/labels` → `/members` → `/tasks` |
| `GET /api/health` | `GET /health/live`, `GET /health/ready` |
| `POST /api/projects/{projectId}/tasks` | `POST /api/v1/workspaces/{ws}/tasks` with `project_id` in body (or `POST /integrations/discord/events` with a token) |
| `GET /api/projects/{projectId}/tasks` | `GET /api/v1/workspaces/{ws}/tasks?project_id=…` or `POST …/tasks/query` |
| `GET /api/tasks/{taskId}` | `GET /api/v1/workspaces/{ws}/tasks/{id}` |
| `PUT /api/tasks/{taskId}` | `PATCH /api/v1/workspaces/{ws}/tasks/{id}` + `expected_version` |
| `DELETE /api/tasks/{taskId}` (permanent) | `DELETE …/tasks/{id}?expected_version=N` (soft, 30-day trash) |
| `POST /api/tasks/{id}/archive` / `restore` | No archive. Use soft delete + `POST …/tasks/{id}/restore`, or a `cancelled` status |
| Task `code` (`PM-12`) | **None.** Use UUID; display `KEY-XXXX` client-side |
| `status` = status id string in project | `status_id` = UUID of a per-project status |
| `isCompleted` on status | `category == "completed"` (or closed = completed/cancelled/duplicate) |
| Default statuses `backlog/planned/in-progress/review/completed` | `Backlog/Todo/In Progress/Done/Cancelled/Duplicate` |
| `priority`: urgent/high/medium/low | `none/low/medium/high/urgent` (default `none`) |
| `labels`: names, defined per project | `label_ids`: UUIDs of **workspace** labels |
| `PUT /api/projects/{id}/workflow` | Status endpoints (§7.4) + label endpoints (§7.5) |
| `PUT /api/projects/{id}/defaults` | No per-project defaults. Choose status/priority in the bot (default status rule §6.5) |
| `startAt` / `deadlineAt` (both required) | `due_start_at` / `due_at` (both optional, nullable) |
| `subtasks[]` (embedded) | Sub-issues = tasks with `parent_task_id` |
| `additionalInfo` | Put in `description` (Markdown) or a comment |
| `attachments` (field `file`, 25 MiB) | `POST …/tasks/{id}/attachments` (field `file`, 25 MiB) |
| Task templates | **No templates** |
| Workspaces `mark`/`color`/`image` | Workspace has only `name` |
| Project `description`/`image`/`sortOrder` | Not present |
| Error `{"error":{"code","message"}}` | `application/problem+json` `{type,title,status,code,detail,instance,request_id}` |
| — | New: comments, relations, assignees, notifications, activity, query DSL, realtime, saved views, pages |
