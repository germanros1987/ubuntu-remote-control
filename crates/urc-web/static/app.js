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
const uploadDir = $('upload-folder-input');
const uploadZ   = $('upload-zone');

function setStatus(msg, cls) {
  statusEl.textContent = msg;
  statusEl.className = 'status ' + (cls || '');
}

function connectedLabel() {
  return remoteHostname ? `connected (${remoteHostname})` : 'connected';
}

// --- VNC ---------------------------------------------------------------

function vncURL() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/ws/vnc`;
}

let rfb;
let lastRemoteClipboard = '';
let userDisconnected = false;
let reconnectAttempt = 0;
let reconnectTimer = null;

function scheduleReconnect() {
  if (userDisconnected) return;
  if (reconnectTimer) return;
  reconnectAttempt += 1;
  const delay = Math.min(1000 * Math.pow(2, reconnectAttempt - 1), 15000);
  setStatus(`reconnecting in ${(delay / 1000).toFixed(0)}s (attempt ${reconnectAttempt})…`, 'err');
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    startVNC();
  }, delay);
}

function startVNC() {
  // Clean up any prior RFB instance (orphan canvas DOM is recreated by noVNC).
  if (rfb) {
    try { rfb.disconnect(); } catch (_) {}
    rfb = null;
    screenEl.innerHTML = '';
  }
  setStatus('connecting…');
  rfb = new RFB(screenEl, vncURL(), { wsProtocols: ['binary'] });
  rfb.scaleViewport  = true;   // fit canvas to local window
  rfb.resizeSession  = false;  // never resize the remote desktop
  rfb.clipViewport   = false;
  rfb.viewOnly       = false;
  rfb.background     = '#111';
  rfb.showDotCursor  = true;   // tiny dot as the local cursor; remote cursor lives in framebuffer

  rfb.addEventListener('connect', () => {
    reconnectAttempt = 0;
    setStatus(remoteHostname ? `connected (${remoteHostname})` : 'connected', 'ok');
  });
  rfb.addEventListener('disconnect', (e) => {
    const clean = e.detail && e.detail.clean;
    if (userDisconnected) {
      setStatus('disconnected', 'err');
      return;
    }
    setStatus(clean ? 'disconnected — reconnecting…' : 'connection lost — reconnecting…', 'err');
    scheduleReconnect();
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
          setTimeout(() => setStatus(connectedLabel(), 'ok'), 2000);
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
  let text;
  try {
    text = await navigator.clipboard.readText();
  } catch (_) {
    setStatus('clipboard read blocked — grant permission and retry', 'err');
    return;
  }
  if (!text) return;
  await pushClipboardToRemote(text);
}

// Writes `text` into the remote's X CLIPBOARD via the agent's HTTP endpoint
// (which runs `xclip` inside the desktop user's session). Falls back to noVNC's
// RFB clipboard channel if the HTTP path errors — some older agents predate
// the endpoint.
async function pushClipboardToRemote(text) {
  try {
    const r = await fetch('/api/clipboard', {
      method: 'POST',
      headers: { 'Content-Type': 'text/plain; charset=utf-8' },
      body: text,
    });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    setStatus('clipboard sent to remote (Ctrl+V to paste there)', 'ok');
    setTimeout(() => setStatus(connectedLabel(), 'ok'), 2500);
  } catch (e) {
    if (rfb) rfb.clipboardPasteFrom(text);
    setStatus('clipboard pushed via VNC fallback', 'ok');
    setTimeout(() => setStatus(connectedLabel(), 'ok'), 2500);
  }
}

$('paste-to-remote').onclick = pasteLocalClipboardToRemote;

$('copy-from-remote').onclick = async () => {
  if (!lastRemoteClipboard) return;
  try {
    await navigator.clipboard.writeText(lastRemoteClipboard);
    $('copy-from-remote').hidden = true;
    setStatus('remote clipboard copied locally', 'ok');
    setTimeout(() => setStatus(connectedLabel(), 'ok'), 2000);
  } catch (_) {
    setStatus('clipboard write blocked — grant permission in browser settings', 'err');
  }
};

// Poll the remote X CLIPBOARD via /api/clipboard. The legacy RFB clipboard
// channel doesn't fire on x0vncserver, so a poll is the simplest reliable
// transport for remote→local copies. Cheap: a single GET every 3s and skipped
// when the tab isn't visible.
let lastPolledRemoteClipboard = '';
async function pollRemoteClipboard() {
  if (document.visibilityState !== 'visible') return;
  try {
    const r = await fetch('/api/clipboard', { cache: 'no-store' });
    if (!r.ok) return;
    const text = await r.text();
    if (!text || text === lastPolledRemoteClipboard) return;
    lastPolledRemoteClipboard = text;
    lastRemoteClipboard = text;
    if (navigator.clipboard) {
      navigator.clipboard.writeText(text).then(
        () => {
          setStatus('remote clipboard copied locally', 'ok');
          setTimeout(() => setStatus(connectedLabel(), 'ok'), 2000);
          $('copy-from-remote').hidden = true;
        },
        () => {
          $('copy-from-remote').hidden = false;
          setStatus('remote sent clipboard — click 📥 Copy to receive', 'ok');
        },
      );
    }
  } catch (_) { /* network blip — try again next tick */ }
}
setInterval(pollRemoteClipboard, 3000);
document.addEventListener('visibilitychange', pollRemoteClipboard);

document.addEventListener('paste', async (e) => {
  // Skip when noVNC has focus on the canvas — we want Cmd-V there to reach the
  // remote OS as a real keystroke, not duplicate via the clipboard channel.
  if (e.target && screenEl.contains(e.target)) return;
  const t = e.clipboardData && e.clipboardData.getData('text/plain');
  if (t) await pushClipboardToRemote(t);
});

$('disconnect').onclick = () => {
  userDisconnected = true;
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null; }
  if (rfb) rfb.disconnect();
};

function setTopbar(visible) {
  document.body.classList.toggle('topbar-hidden', !visible);
  $('show-topbar').hidden = visible;
}
$('hide-topbar').onclick = () => setTopbar(false);
$('show-topbar').onclick = () => setTopbar(true);

// Fetch the remote hostname once so status text can read "connected (sa-grs)".
let remoteHostname = '';
fetch('/api/host').then(r => r.ok ? r.json() : null).then(j => {
  if (j && j.hostname) {
    remoteHostname = j.hostname;
    if (statusEl.textContent === 'connected') setStatus(connectedLabel(), 'ok');
  }
}).catch(() => {});

// Page becoming visible (laptop wake, tab refocus) is a good moment to retry
// immediately instead of waiting out the backoff.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && !userDisconnected && reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
    startVNC();
  }
});
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
  // The agent's files_root maps to "/" in the URL space; show /home + cwd so
  // the user knows which absolute path their uploads will land in.
  cwdEl.textContent = '/home' + (path ? '/' + path : '');
  renderList(entries);
}

function renderList(entries) {
  listEl.innerHTML = '';
  if (cwd) {
    const up = row('..', true, () => listDir(parentOf(cwd)));
    listEl.appendChild(up);
  }
  for (const e of entries) {
    const fullPath = cwd ? `${cwd}/${e.name}` : e.name;
    const click = e.is_dir ? () => listDir(fullPath) : null;
    const r = row(e.name, e.is_dir, click);
    const dl = document.createElement('a');
    dl.className = 'download';
    if (e.is_dir) {
      dl.textContent = '↓zip';
      dl.href = `/api/download-zip/${encodeURI(fullPath)}`;
      dl.title = 'Download folder as .zip';
    } else {
      dl.textContent = '↓';
      dl.href = `/api/download/${encodeURI(fullPath)}`;
      dl.download = e.name;
      const sz = document.createElement('span');
      sz.className = 'size';
      sz.textContent = humanSize(e.size);
      r.appendChild(sz);
    }
    r.appendChild(dl);
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

// Uploads `file` to the remote. `relPath` lets folder uploads preserve their
// directory structure: e.g. a folder "proj" with "src/main.rs" inside is
// uploaded with relPath="proj/src/main.rs", and the server's `create_dir_all`
// builds the parents on the remote.
async function uploadFile(file, relPath) {
  const subpath = relPath || file.name;
  const dest = (cwd ? `${cwd}/` : '') + subpath;
  const form = new FormData();
  form.append('file', file, file.name);

  const progress = $('upload-progress');
  progress.hidden = false;
  progress.textContent = `Uploading ${subpath}… 0%`;
  showError('');

  await new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', `/api/upload/${encodeURI(dest)}`);
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) {
        const pct = ((e.loaded / e.total) * 100).toFixed(0);
        progress.textContent = `Uploading ${subpath}… ${pct}% (${humanSize(e.loaded)}/${humanSize(e.total)})`;
      }
    };
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        progress.textContent = `Uploaded ${subpath}`;
        setTimeout(() => { progress.hidden = true; }, 1500);
      } else {
        showError(`Upload ${subpath} failed: HTTP ${xhr.status} ${xhr.responseText || ''}`);
        progress.hidden = true;
      }
      resolve();
    };
    xhr.onerror = () => {
      showError(`Upload ${subpath} failed: network error`);
      progress.hidden = true;
      resolve();
    };
    xhr.send(form);
  });
}

async function uploadFileList(fileList) {
  for (const f of fileList) {
    // `webkitRelativePath` is non-empty for folder uploads (input[webkitdirectory]
    // or drag-dropped directories) and carries the path inside the chosen root.
    await uploadFile(f, f.webkitRelativePath || f.name);
  }
  await listDir(cwd);
}

uploadIn.addEventListener('change', async (e) => {
  await uploadFileList(e.target.files);
  uploadIn.value = '';
});

uploadDir.addEventListener('change', async (e) => {
  await uploadFileList(e.target.files);
  uploadDir.value = '';
});

['dragenter', 'dragover'].forEach(ev =>
  uploadZ.addEventListener(ev, (e) => { e.preventDefault(); uploadZ.classList.add('drag'); }));
['dragleave', 'drop'].forEach(ev =>
  uploadZ.addEventListener(ev, (e) => { e.preventDefault(); uploadZ.classList.remove('drag'); }));

uploadZ.addEventListener('drop', async (e) => {
  // Prefer the DataTransferItemList: it can walk dropped folders via
  // webkitGetAsEntry, which `dataTransfer.files` cannot. Fall back to the
  // flat file list when entries aren't available.
  const items = e.dataTransfer.items;
  if (items && items.length && items[0].webkitGetAsEntry) {
    const collected = [];
    for (const item of items) {
      const entry = item.webkitGetAsEntry && item.webkitGetAsEntry();
      if (entry) await walkEntry(entry, '', collected);
    }
    await uploadFileList(collected);
  } else {
    await uploadFileList(e.dataTransfer.files);
  }
});

// Recursively walk a FileSystemEntry (from drag-drop) and push File objects
// with a synthetic `webkitRelativePath` so uploadFile preserves the layout.
function walkEntry(entry, prefix, out) {
  return new Promise((resolve) => {
    if (entry.isFile) {
      entry.file((file) => {
        try {
          Object.defineProperty(file, 'webkitRelativePath', {
            value: prefix + entry.name,
            configurable: true,
          });
        } catch (_) { /* property already set in some browsers */ }
        out.push(file);
        resolve();
      }, () => resolve());
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      const read = () => {
        reader.readEntries(async (entries) => {
          if (!entries.length) return resolve();
          for (const e of entries) await walkEntry(e, prefix + entry.name + '/', out);
          read();
        }, () => resolve());
      };
      read();
    } else {
      resolve();
    }
  });
}

function setPanelOpen(open) {
  panelEl.hidden = !open;
  $('toggle-files').setAttribute('aria-pressed', open ? 'true' : 'false');
  if (open) listDir(cwd);
}

$('toggle-files').onclick = () => setPanelOpen(panelEl.hidden);
$('close-files').onclick = () => setPanelOpen(false);

// --- boot --------------------------------------------------------------

startVNC();
