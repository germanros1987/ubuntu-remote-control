// URC unified web app: noVNC client + files panel.
//
// noVNC is vendored at /novnc/core/. The page loads as an ES module so we can
// `import` RFB directly without a build step.

import RFB from '/novnc/core/rfb.js';

const $ = (id) => document.getElementById(id);

const screenEl  = $('screen');
const statusEl  = $('status');
const panelEl   = $('files-panel');
const listEl    = $('files-list');
const cwdEl     = $('cwd');
const errEl     = $('files-error');
const uploadIn  = $('upload-input');
const uploadZ   = $('upload-zone');

function setStatus(msg, cls) {
  statusEl.textContent = msg;
  statusEl.className = 'status ' + (cls || '');
}

// --- VNC ---------------------------------------------------------------

function vncURL() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/ws/vnc`;
}

let rfb;
let lastRemoteClipboard = '';

function startVNC() {
  setStatus('connecting…');
  rfb = new RFB(screenEl, vncURL(), { wsProtocols: ['binary'] });
  rfb.scaleViewport  = true;   // fit canvas to local window
  rfb.resizeSession  = false;  // never resize the remote desktop
  rfb.clipViewport   = false;
  rfb.viewOnly       = false;
  rfb.background     = '#111';
  rfb.showDotCursor  = true;   // tiny dot as the local cursor; remote cursor lives in framebuffer

  rfb.addEventListener('connect',     () => setStatus('connected', 'ok'));
  rfb.addEventListener('disconnect',  (e) => {
    const clean = e.detail && e.detail.clean;
    setStatus(clean ? 'disconnected' : 'connection lost', 'err');
  });
  rfb.addEventListener('credentialsrequired', () => {
    rfb.sendCredentials({ password: '' }); // server uses SecurityTypes=None
  });
  rfb.addEventListener('clipboard', (e) => {
    const text = e.detail && e.detail.text;
    if (!text) return;
    lastRemoteClipboard = text;
    // Try a silent write first — browsers allow it shortly after a user gesture.
    if (navigator.clipboard) {
      navigator.clipboard.writeText(text).then(
        () => {
          setStatus('remote clipboard copied locally', 'ok');
          setTimeout(() => setStatus('connected', 'ok'), 2000);
          $('copy-from-remote').hidden = true;
        },
        () => {
          // Browser blocked us — surface the "Copy from Remote" button so the
          // user can grant the gesture explicitly.
          $('copy-from-remote').hidden = false;
          setStatus('remote sent clipboard — click 📥 Copy to receive', 'ok');
        },
      );
    }
  });
}

// --- Local → remote clipboard --------------------------------------------
//
// Browsers only expose the system clipboard on an explicit user gesture, so
// we offer two paths:
//
//   1. The "Paste" button in the toolbar — reads navigator.clipboard.readText()
//      and pushes the text into the remote's X clipboard. The user then pastes
//      inside the remote desktop with the remote OS's paste shortcut (Ctrl+V
//      on Linux).
//   2. A page-wide `paste` event listener — when the user presses Cmd-V / Ctrl-V
//      while the URC tab is focused (but not while typing into the VNC canvas
//      itself, where noVNC eats the keystroke), the browser fires a paste event
//      with the clipboard contents. We forward it the same way.
async function pasteLocalClipboardToRemote() {
  if (!rfb) return;
  try {
    const text = await navigator.clipboard.readText();
    if (text) {
      rfb.clipboardPasteFrom(text);
      setStatus('clipboard sent to remote (Ctrl+V to paste there)', 'ok');
      setTimeout(() => setStatus('connected', 'ok'), 2500);
    }
  } catch (e) {
    setStatus('clipboard read blocked — grant permission and retry', 'err');
  }
}

$('paste-to-remote').onclick = pasteLocalClipboardToRemote;

$('copy-from-remote').onclick = async () => {
  if (!lastRemoteClipboard) return;
  try {
    await navigator.clipboard.writeText(lastRemoteClipboard);
    $('copy-from-remote').hidden = true;
    setStatus('remote clipboard copied locally', 'ok');
    setTimeout(() => setStatus('connected', 'ok'), 2000);
  } catch (_) {
    setStatus('clipboard write blocked — grant permission in browser settings', 'err');
  }
};

document.addEventListener('paste', (e) => {
  // Skip when noVNC has focus on the canvas — we want Cmd-V there to reach the
  // remote OS as a real keystroke, not duplicate via the clipboard channel.
  if (e.target && screenEl.contains(e.target)) return;
  const t = e.clipboardData && e.clipboardData.getData('text/plain');
  if (rfb && t) {
    rfb.clipboardPasteFrom(t);
    setStatus('clipboard sent to remote (Ctrl+V to paste there)', 'ok');
    setTimeout(() => setStatus('connected', 'ok'), 2500);
  }
});

$('disconnect').onclick = () => { if (rfb) rfb.disconnect(); };
$('toggle-fullscreen').onclick = () => {
  if (!document.fullscreenElement) document.documentElement.requestFullscreen();
  else document.exitFullscreen();
};

// --- Files -------------------------------------------------------------

let cwd = '';

function showError(msg) {
  errEl.textContent = msg;
  errEl.hidden = !msg;
}

async function listDir(path) {
  showError('');
  const apiPath = path ? `/api/list/${encodeURI(path)}` : '/api/list';
  const r = await fetch(apiPath);
  if (!r.ok) { showError(`List failed: ${r.status}`); return; }
  const entries = await r.json();
  cwd = path;
  cwdEl.textContent = '/' + (path || '');
  renderList(entries);
}

function renderList(entries) {
  listEl.innerHTML = '';
  if (cwd) {
    const up = row('..', true, () => listDir(parentOf(cwd)));
    listEl.appendChild(up);
  }
  for (const e of entries) {
    const click = e.is_dir
      ? () => listDir(cwd ? `${cwd}/${e.name}` : e.name)
      : null;
    const r = row(e.name, e.is_dir, click);
    if (!e.is_dir) {
      const dl = document.createElement('a');
      dl.textContent = '↓';
      dl.className = 'download';
      dl.href = `/api/download/${encodeURI(cwd ? `${cwd}/${e.name}` : e.name)}`;
      dl.download = e.name;
      r.appendChild(dl);
      const sz = document.createElement('span');
      sz.className = 'size';
      sz.textContent = humanSize(e.size);
      r.appendChild(sz);
    }
    listEl.appendChild(r);
  }
}

function row(name, isDir, onClick) {
  const r = document.createElement('div');
  r.className = 'row' + (isDir ? ' dir' : '');
  const n = document.createElement('span');
  n.className = 'name';
  n.textContent = (isDir ? '📁 ' : '📄 ') + name;
  if (onClick) { n.style.cursor = 'pointer'; n.onclick = onClick; }
  r.appendChild(n);
  return r;
}

function parentOf(p) {
  const i = p.lastIndexOf('/');
  return i < 0 ? '' : p.slice(0, i);
}

function humanSize(n) {
  const units = ['B', 'K', 'M', 'G', 'T'];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i === 0 ? 0 : 1)}${units[i]}`;
}

async function uploadFile(file) {
  const dest = (cwd ? `${cwd}/` : '') + file.name;
  const form = new FormData();
  form.append('file', file, file.name);
  const r = await fetch(`/api/upload/${encodeURI(dest)}`, { method: 'POST', body: form });
  if (!r.ok) { showError(`Upload ${file.name} failed: ${r.status}`); return; }
  await listDir(cwd);
}

uploadIn.addEventListener('change', async (e) => {
  for (const f of e.target.files) await uploadFile(f);
  uploadIn.value = '';
});

['dragenter', 'dragover'].forEach(ev =>
  uploadZ.addEventListener(ev, (e) => { e.preventDefault(); uploadZ.classList.add('drag'); }));
['dragleave', 'drop'].forEach(ev =>
  uploadZ.addEventListener(ev, (e) => { e.preventDefault(); uploadZ.classList.remove('drag'); }));
uploadZ.addEventListener('drop', async (e) => {
  for (const f of e.dataTransfer.files) await uploadFile(f);
});

$('toggle-files').onclick = () => {
  panelEl.hidden = !panelEl.hidden;
  if (!panelEl.hidden) listDir(cwd);
};

// --- boot --------------------------------------------------------------

startVNC();
