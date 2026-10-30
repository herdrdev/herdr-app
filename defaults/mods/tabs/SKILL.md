---
name: herdr-titles
description: Inspect Herdr desktop titles, pin a manual tab title, or restore automatic naming through the local API or MCP tools.
---

# Herdr automatic titles

Unnamed tabs follow useful agent terminal titles, then program/folder fallbacks.
Manual names always win; an empty name resumes automatic titles. Spaces keep their
existing repository/folder naming. No LLM calls or transcript uploads are used.

| Action | JSON API | MCP tool |
|---|---|---|
| inspect titles | GET /v1/titles | herdr_titles_list |
| Rename, Use automatic title | POST /v1/tab-title | herdr_tab_title_set |

Run with Node 22+: `node /path/to/mods/tabs/agent.mjs` (MCP stdio, protocol
2025-11-25), or `node /path/to/mods/tabs/agent.mjs --http 0` (local JSON API).
The HTTP command prints its loopback URL; `GET /v1/tools` lists the same schemas.
HTTP POST bodies are JSON arguments from the tool schemas below.
Set HERDR_APP_HOME to target an isolated home; XDG_CONFIG_HOME is also respected.
HERDR_APP_BIN can select the app executable. Herdr must already be running.

Title inspection returns a last-rendered snapshot with updated_at and data.
A shipped tab bar must have rendered; changes can take a frame plus a disk write.
Retry after a catching-up error. Do not treat updated_at as a heartbeat.
The title snapshot is mod-owned shared state, never a new core document field.
Custom layouts without a shipped tab bar do not publish it.

Rename/reset maps to the existing app API: `herdr-app do tab.rename` with
`{"id":"tb1","name":"Fix login"}` or `{"id":"tb1","name":""}`.
Use IDs from inspection, including machine prefixes, not titles or positions.
Check tool errors; do not open or restart a window to make inspection work.
After a write, inspect again to confirm the rendered mode and title.

Agent prompt: "Inspect my Herdr titles using herdr_titles_list. For the tab I
identify, call herdr_tab_title_set with its exact id and an empty name to restore
automatic titles, or my requested name to pin it. Inspect again and report the
result. Preserve all other names and do not restart any app or terminal."

Read [llms-full.txt](llms-full.txt) for the generated argument schemas.
