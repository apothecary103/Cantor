// "Connect" modal: snippets for wiring an AI agent to this Cantor instance over MCP.
import { isMock } from './api.js';
import { h, modal, copyButton } from './ui.js';

export function openConnect() {
  const origin = isMock() || location.protocol === 'file:' ? 'http://127.0.0.1:7878' : location.origin;
  const desktop = '{\n  "mcpServers": {\n    "cantor": { "command": "cantor", "args": ["mcp"] }\n  }\n}';
  const snippets = [
    ['Claude Code', 'claude mcp add cantor -- cantor mcp'],
    ['Claude Desktop (claude_desktop_config.json) or any stdio MCP client', desktop],
    ['Streamable HTTP endpoint of this server', `${origin}/mcp`],
    ['Claude Code over HTTP, sharing this session', `claude mcp add --transport http cantor ${origin}/mcp`],
  ];
  modal('Connect an agent', h('div', { class: 'connect' },
    h('p', { class: 'lede' }, 'Cantor speaks MCP. An agent edits the same project you see here, with the same undoable ops; its edits appear live.'),
    snippets.map(([label, text]) => h('div', { class: 'snippet' },
      h('div', { class: 'snippet-head' }, h('span', {}, label), copyButton(() => text)),
      h('pre', { class: 'code' }, text))),
    h('div', { class: 'snippet-head' }, h('span', {}, 'How an agent makes a cover')),
    h('ol', { class: 'steps' },
      h('li', {}, h('code', {}, 'import'), ' the MIDI, lyrics and original vocal (as reference); pick a singer with ', h('code', {}, 'list_voicebanks'), '.'),
      h('li', {}, h('code', {}, 'tune'), ' with auto_pitch, auto_dynamics, breaths and match_reference, then ', h('code', {}, 'render'), ' and read the report.'),
      h('li', {}, 'Fix what the numbers flag with ', h('code', {}, 'apply_ops'), ', re-render, then ', h('code', {}, 'export'), '. The guide is at ', h('code', {}, 'cantor://guide'), '.'))));
}
