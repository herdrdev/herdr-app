import { readFile, writeFile } from 'node:fs/promises';
import { tools } from '../defaults/mods/tabs/agent.mjs';
const folder = new URL('../defaults/mods/tabs/', import.meta.url);
const table = tools.map(t => `| ${t.uiActions.join(', ')} | ${t.http.method} ${t.http.path} | ${t.name} |`).join('\n');
const overview = `# Herdr automatic titles\n\nUnnamed tabs follow useful agent terminal titles, then program/folder fallbacks.\nManual names always win; an empty name resumes automatic titles. Spaces keep their\nexisting repository/folder naming. No LLM calls or transcript uploads are used.\n\n| Action | JSON API | MCP tool |\n|---|---|---|\n${table}\n`;
const workflow = `\nRun with Node 22+: \`node /path/to/mods/tabs/agent.mjs\` (MCP stdio, protocol\n2025-11-25), or \`node /path/to/mods/tabs/agent.mjs --http 0\` (local JSON API).\nThe HTTP command prints its loopback URL; \`GET /v1/tools\` lists the same schemas.\nHTTP POST bodies are JSON arguments from the tool schemas below.\nSet HERDR_APP_HOME to target an isolated home; XDG_CONFIG_HOME is also respected.\nHERDR_APP_BIN can select the app executable. Herdr must already be running.\n\nTitle inspection returns a last-rendered snapshot with updated_at and data.\nA shipped tab bar must have rendered; changes can take a frame plus a disk write.\nRetry after a catching-up error. Do not treat updated_at as a heartbeat.\nThe title snapshot is mod-owned shared state, never a new core document field.\nCustom layouts without a shipped tab bar do not publish it.\n\nRename/reset maps to the existing app API: \`herdr-app do tab.rename\` with\n\`{"id":"tb1","name":"Fix login"}\` or \`{"id":"tb1","name":""}\`.\nUse IDs from inspection, including machine prefixes, not titles or positions.\nCheck tool errors; do not open or restart a window to make inspection work.\nAfter a write, inspect again to confirm the rendered mode and title.\n\nAgent prompt: "Inspect my Herdr titles using herdr_titles_list. For the tab I\nidentify, call herdr_tab_title_set with its exact id and an empty name to restore\nautomatic titles, or my requested name to pin it. Inspect again and report the\nresult. Preserve all other names and do not restart any app or terminal."\n`;
const schemas = '\n## Tool schemas\n\n' + tools.map(t => `### ${t.name}\n\n${t.description}\n\n\`\`\`json\n${JSON.stringify(t.inputSchema, null, 2)}\n\`\`\`\n`).join('\n');
const files = {
  'llms.txt': overview + '\nSee [full API reference](llms-full.txt) and [agent workflow](SKILL.md).\n',
  'llms-full.txt': overview + workflow + schemas,
  'SKILL.md': '---\nname: herdr-titles\ndescription: Inspect Herdr desktop titles, pin a manual tab title, or restore automatic naming through the local API or MCP tools.\n---\n\n' + overview + workflow + '\nRead [llms-full.txt](llms-full.txt) for the generated argument schemas.\n',
};
for (const [name, text] of Object.entries(files)) {
  const url = new URL(name, folder);
  if (process.argv.includes('--check')) {
    if (await readFile(url, 'utf8') !== text) throw new Error(`${name} is stale; run node scripts/title-agent-docs.mjs`);
  } else await writeFile(url, text);
}
