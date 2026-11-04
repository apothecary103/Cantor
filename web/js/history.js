// History tab: every undo step with who made it (you, an agent over MCP, the CLI), and named checkpoints.
// Clicking a step undoes or redoes to it; restoring a checkpoint is itself one undoable step.
import { S, on, call, loadHistory, sourceName } from './state.js';
import { h, promptText } from './ui.js';

let edits, cps;

export function initHistory(el) {
  edits = h('div', { class: 'rlist' });
  cps = h('div', { class: 'rlist' });
  el.append(h('div', { class: 'scroll' },
    h('div', { class: 'psec' },
      h('div', { class: 'psec-title' }, h('span', {}, 'Checkpoints'),
        h('button', { class: 'linkish', onclick: saveCheckpoint, title: 'Snapshot the project under a name' }, 'Save…')),
      cps),
    h('div', { class: 'psec' },
      h('div', { class: 'psec-title' }, h('span', {}, 'Edits'), h('span', { class: 'scope' }, 'click to go back or forward')),
      edits)));
  on('history', render);
  render();
}

export async function saveCheckpoint() {
  const name = (await promptText('Save checkpoint', 'Name', `take ${S.history.checkpoints.length + 1}`))?.trim();
  if (!name) return;
  await call('api/checkpoint', { name }, `Checkpoint “${name}” saved`);
  loadHistory();
}

const go = (steps) => {
  if (steps) call(steps > 0 ? 'api/undo' : 'api/redo', { steps: Math.abs(steps) });
};

function stepRow(step, cls, onclick, title) {
  const who = step.source && step.source !== 'http' ? h('span', { class: `who ${step.source}` }, sourceName(step.source)) : null;
  return h('button', { class: `rrow ${cls}`, onclick, title }, h('span', { class: 'rmain' }, step.label || 'Edit'), who);
}

function render() {
  const { undo, redo, checkpoints } = S.history;
  cps.replaceChildren(...(checkpoints.length ? checkpoints.map((name) => h('div', { class: 'rrow static' },
    h('span', { class: 'rmain' }, name),
    h('button', { class: 'linkish', title: 'Return to this checkpoint (undoable)', onclick: () => call('api/restore', { name }, `Restored “${name}”`) }, 'Restore')))
    : [h('p', { class: 'empty' }, 'None yet. Agents save one before risky passes; you can too.')]));

  const n = undo.length;
  const rows = [stepRow({ label: 'Opened' }, n ? '' : 'current', () => go(n), 'Undo everything')];
  undo.forEach((s, i) => rows.push(stepRow(s, i === n - 1 ? 'current' : '', () => go(n - 1 - i), i === n - 1 ? 'Current state' : 'Undo to here')));
  redo.forEach((s, j) => rows.push(stepRow(s, 'undone', () => go(-(j + 1)), 'Redo to here')));
  edits.replaceChildren(...rows);
  edits.querySelector('.current')?.scrollIntoView({ block: 'nearest' });
}
