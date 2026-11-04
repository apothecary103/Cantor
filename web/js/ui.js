// Small DOM toolkit: element builder, drag-value fields, menus, modals, icons.

export function h(tag, props = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v == null || v === false) continue;
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'class') el.className = v;
    else if (k === 'html') el.innerHTML = v;
    else if (k in el && typeof v !== 'string') el[k] = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  el.append(...kids.flat().filter((c) => c != null && c !== false));
  return el;
}

const PATHS = {
  play: 'M4 2.5v11l9-5.5z',
  pause: 'M4 3h3v10H4zM9 3h3v10H9z',
  stop: 'M3.5 3.5h9v9h-9z',
  ret: 'M3 3h1.6v10H3zM13 3v10L6 8z',
  loop: 'M4 5h7.5V3l3 3-3 3V7H5v2H3V6a1 1 0 0 1 1-1zm8 6H4.5v2l-3-3 3-3v2H11V7h2v3a1 1 0 0 1-1 1z',
  select: 'M3 2l9.5 6.2-4.3.7 2.6 4.6-1.7 1-2.6-4.6L3 13z',
  draw: 'M11.3 1.9l2.8 2.8-8.6 8.6L2 14l.7-3.5zM10 4.6l1.4 1.4',
  knife: 'M2 13.5L12.5 3l1.5 1.5-7 8-2.5-.5-1.5 2z',
  pen: 'M1.5 11c2-5 4-6.5 5.5-3s3.5 2 7-4.5l.9.5c-3.5 7-6.6 8.3-8.8 4.2-1-2-2-1.7-3.7 3.2z',
  plus: 'M7.25 3h1.5v4.25H13v1.5H8.75V13h-1.5V8.75H3v-1.5h4.25z',
  x: 'M4.1 3l3.9 3.9L11.9 3 13 4.1 9.1 8l3.9 3.9-1.1 1.1L8 9.1 4.1 13 3 11.9 6.9 8 3 4.1z',
  copy: 'M5 2h7a1 1 0 0 1 1 1v8h-1.5V3.5H5zM3 5h6.5a1 1 0 0 1 1 1v7.5a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zm.5 1.5v6.5H9V6.5z',
};
export const icon = (name) =>
  h('span', { class: 'icon', html: `<svg viewBox="0 0 16 16" width="14" height="14"><path d="${PATHS[name]}"/></svg>` });

export const fixed = (v, digits) => (Math.round(v * 10 ** digits) / 10 ** digits).toFixed(digits);

/**
 * egui-style DragValue: drag horizontally to change, shift for fine steps,
 * double-click (or Enter) to type, alt-click to reset to `reset`.
 * `onCommit(value)` fires once per gesture; `set(value, mixed)` updates the display.
 */
export function dragValue(o) {
  const { min = -Infinity, max = Infinity, step = 1, digits = 0, unit = '', fmt, nullText, onCommit } = o;
  const el = h('span', { class: `drag num${o.class ? ' ' + o.class : ''}`, tabIndex: 0, title: o.title });
  let value = o.value ?? null, mixed = false, editing = false;
  const clamp = (v) => Math.min(max, Math.max(min, v));
  const text = () => mixed ? '—' : value == null ? (nullText ?? '—') : fmt ? fmt(value) : fixed(value, digits) + unit;
  const show = () => { if (!editing) el.textContent = text(); };
  el.set = (v, isMixed = false) => { value = v; mixed = isMixed; show(); };
  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || editing) return;
    e.preventDefault();
    el.focus();
    if (e.altKey && o.reset !== undefined) { value = o.reset; mixed = false; show(); onCommit(value); return; }
    el.setPointerCapture(e.pointerId);
    let acc = mixed || value == null ? (o.reset ?? clamp(0)) : value, moved = 0;
    const move = (ev) => {
      moved += Math.abs(ev.movementX);
      if (moved < 3) return;
      acc = clamp(acc + ev.movementX * step * (ev.shiftKey ? 0.1 : 1));
      value = Number(fixed(acc, digits));
      mixed = false;
      el.classList.add('active');
      show();
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      el.classList.remove('active');
      if (moved >= 3) onCommit(value);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  });
  const edit = () => {
    editing = true;
    const input = h('input', { class: 'drag-input', value: mixed || value == null ? '' : fixed(value, digits) });
    el.textContent = '';
    el.append(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (ok) => {
      if (done) return;
      done = true;
      editing = false;
      const raw = input.value.trim();
      if (ok) {
        const v = o.parse ? o.parse(raw) : raw === '' && nullText ? null : parseFloat(raw);
        if (v === null || Number.isFinite(v)) { value = v === null ? null : clamp(v); mixed = false; onCommit(value); }
      }
      show();
      el.focus();
    };
    input.addEventListener('keydown', (e) => {
      e.stopPropagation();
      if (e.key === 'Enter') finish(true);
      if (e.key === 'Escape') finish(false);
    });
    input.addEventListener('blur', () => finish(true));
  };
  el.addEventListener('dblclick', edit);
  el.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !editing) { e.preventDefault(); edit(); } });
  show();
  return el;
}

/** Single-line text field committing on Enter/blur; shows "—" placeholder when mixed. */
export function textField(o) {
  const el = h('input', { class: 'field', spellcheck: 'false', placeholder: o.placeholder || '' });
  let orig = '';
  el.set = (v, mixed = false) => {
    if (document.activeElement === el) return;
    orig = mixed ? '' : v ?? '';
    el.value = orig;
    el.placeholder = mixed ? '—' : o.placeholder || '';
  };
  el.addEventListener('keydown', (e) => {
    e.stopPropagation();
    if (e.key === 'Enter') el.blur();
    if (e.key === 'Escape') { el.value = orig; el.blur(); }
  });
  el.addEventListener('change', () => o.onCommit(el.value));
  return el;
}

export function selectBox(options, value, onChange, cls = '') {
  const el = h('select', { class: `select ${cls}`, onchange: () => onChange(el.value) },
    options.map(([v, label]) => h('option', { value: v }, label)));
  el.value = value;
  return el;
}

export function toggleBtn(label, title, onClick) {
  const el = h('button', { class: 'tog', title, onclick: onClick }, label);
  el.set = (on) => el.classList.toggle('on', !!on);
  return el;
}

// ---- menus ---------------------------------------------------------------------------
/** menus: [{ label, items: [{ label, key, run, checked?(), enabled?() } | '-'] }] */
export function menuBar(root, menus) {
  let open = null;
  const close = () => { open?.classList.remove('open'); open = null; };
  for (const m of menus) {
    const list = h('div', { class: 'menu-list' });
    const top = h('div', { class: 'menu' }, h('button', { class: 'menu-title' }, m.label), list);
    const build = () => {
      list.replaceChildren(...m.items.map((it) => {
        if (it === '-') return h('div', { class: 'menu-sep' });
        const enabled = it.enabled ? it.enabled() : true;
        return h('button', {
          class: `menu-item${enabled ? '' : ' disabled'}`,
          onclick: () => { if (!enabled) return; close(); it.run(); },
        }, h('span', { class: 'check' }, it.checked?.() ? '✓' : ''), h('span', { class: 'label' }, it.label),
        h('span', { class: 'key' }, it.key || ''));
      }));
    };
    const show = () => { close(); build(); top.classList.add('open'); open = top; };
    top.firstChild.addEventListener('pointerdown', (e) => { e.stopPropagation(); open === top ? close() : show(); });
    top.firstChild.addEventListener('pointerenter', () => { if (open && open !== top) show(); });
    root.append(top);
  }
  document.addEventListener('pointerdown', (e) => { if (open && !open.contains(e.target)) close(); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); }, true);
}

// ---- modal --------------------------------------------------------------------------------
export function modal(title, body, actions = []) {
  const back = h('div', { class: 'modal-back' });
  const close = () => { back.remove(); document.removeEventListener('keydown', key, true); };
  const key = (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(); } };
  const box = h('div', { class: 'modal', role: 'dialog' },
    h('div', { class: 'modal-head' }, h('span', {}, title), h('button', { class: 'icon-btn', title: 'Close', onclick: close }, icon('x'))),
    h('div', { class: 'modal-body' }, body),
    actions.length ? h('div', { class: 'modal-foot' }, actions.map(([label, fn, primary]) =>
      h('button', { class: `btn${primary ? ' primary' : ''}`, onclick: () => { if (fn() !== false) close(); } }, label))) : null);
  back.append(box);
  back.addEventListener('pointerdown', (e) => { if (e.target === back) close(); });
  document.addEventListener('keydown', key, true);
  document.body.append(back);
  box.querySelector('input')?.focus();
  return close;
}

/** Prompts for one line of text; resolves null when cancelled. */
export function promptText(title, label, value = '') {
  return new Promise((resolve) => {
    let result = null;
    const input = h('input', { class: 'field wide', value, spellcheck: 'false' });
    const close = modal(title, h('label', { class: 'prompt' }, h('span', {}, label), input), [
      ['Cancel', () => {}],
      ['OK', () => { result = input.value; }, true],
    ]);
    input.addEventListener('keydown', (e) => {
      e.stopPropagation();
      if (e.key === 'Enter') { result = input.value; close(); }
    });
    new MutationObserver((_, obs) => { if (!input.isConnected) { obs.disconnect(); resolve(result); } })
      .observe(document.body, { childList: true });
  });
}

export function copyButton(getText) {
  const b = h('button', { class: 'btn small', title: 'Copy to clipboard' }, icon('copy'), h('span', {}, 'Copy'));
  b.addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(getText()); } catch { /* clipboard blocked */ }
    b.lastChild.textContent = 'Copied';
    setTimeout(() => { b.lastChild.textContent = 'Copy'; }, 1200);
  });
  return b;
}

// ---- tabs ------------------------------------------------------------------------------------
/** A panel with a tab strip: defs [[id, label]]. Returns { panes, show(id), label(id, text), current() }. */
export function tabs(root, defs) {
  const strip = h('div', { class: 'panel-head tabs', role: 'tablist' });
  const panes = {}, btns = {};
  let cur = null;
  const show = (id) => {
    cur = id;
    for (const [k] of defs) {
      btns[k].classList.toggle('on', k === id);
      btns[k].setAttribute('aria-selected', String(k === id));
      panes[k].classList.toggle('hidden', k !== id);
    }
  };
  for (const [id, label] of defs) {
    btns[id] = h('button', { class: 'ptab', role: 'tab', onclick: () => show(id) }, label);
    panes[id] = h('div', { class: 'pane', role: 'tabpanel' });
    strip.append(btns[id]);
  }
  root.append(strip, ...Object.values(panes));
  show(defs[0][0]);
  return { panes, show, label: (id, text) => { btns[id].textContent = text; }, current: () => cur };
}
