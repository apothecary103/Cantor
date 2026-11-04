// HTTP client for the Cantor JSON API, with a transparent in-browser mock.
import * as mock from './mock.js';

let mocked = new URLSearchParams(location.search).has('mock');

// The page is served at `/` or under `/web/`; the API always lives at the server root.
const ROOT = (() => {
  const p = location.pathname, i = p.lastIndexOf('/web/');
  return i >= 0 ? p.slice(0, i + 1) : p.replace(/[^/]*$/, '');
})();

export const url = (path) => ROOT + path;
export const isMock = () => mocked;

/** Identifies this tab; the server echoes it in events so the UI can tell its own edits from an agent's. */
export const CLIENT = `web-${Math.random().toString(36).slice(2, 10)}`;

export class ApiError extends Error {
  constructor(code, message, diagnostics = []) {
    super(message);
    this.code = code;
    this.diagnostics = diagnostics;
  }
}

async function request(method, path, body, binary = false) {
  if (mocked) return mock.handle(method, path, body, CLIENT);
  const init = { method, headers: { 'x-cantor-client': CLIENT } };
  if (body instanceof Blob) { init.body = body; init.headers['content-type'] = 'application/octet-stream'; }
  else if (body !== undefined) {
    init.headers['content-type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const res = await fetch(url(path), init);
  const json = (res.headers.get('content-type') || '').includes('json');
  if (!res.ok) {
    const e = json ? (await res.json()).error || {} : {};
    throw new ApiError(e.code || res.status, e.message || `${method} ${path}: ${res.status}`, e.diagnostics);
  }
  if (binary) return res.arrayBuffer();
  return json ? res.json() : res.text();
}

export const get = (path) => request('GET', path);
export const getBinary = (path) => request('GET', path, undefined, true);
export const post = (path, body = {}) => request('POST', path, body);

/** First contact: falls back to the mock when no server answers. */
export async function connect() {
  try {
    return await get('api/state');
  } catch (e) {
    if (mocked) throw e;
    console.info('cantor: no server at', url('api/state'), '- using the in-browser mock');
    mocked = true;
    return get('api/state');
  }
}

export function imageUrl(voicebankId) {
  return mocked ? mock.imageUrl(voicebankId) : url(`api/voicebanks/${encodeURIComponent(voicebankId)}/image`);
}

/** Subscribes to server events; `onStatus` receives 'live' | 'mock' | 'reconnecting'. */
export function subscribe(onEvent, onStatus) {
  if (mocked) {
    onStatus('mock');
    mock.events.addEventListener('event', (e) => onEvent(e.detail));
    return;
  }
  const es = new EventSource(url('api/events'));
  const handle = (e) => {
    try { onEvent(JSON.parse(e.data)); } catch { /* keep-alive or non-JSON */ }
  };
  es.onmessage = handle;
  for (const t of ['revision', 'render', 'progress', 'checkpoint']) es.addEventListener(t, handle);
  es.onopen = () => onStatus('live');
  es.onerror = () => onStatus('reconnecting');
}

/** Triggers a browser download of an API resource. */
export async function download(path, filename) {
  const a = document.createElement('a');
  a.download = filename;
  if (mocked) {
    const data = await getBinary(path);
    a.href = URL.createObjectURL(new Blob([data]));
    setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  } else {
    a.href = url(path);
  }
  a.click();
}
