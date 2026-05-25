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
  // A fresh RFB starts at fit; clear any stale local-magnify state so a
  // reconnect doesn't think we're still zoomed (defined below; hoisted vars).
  zoomLevel = 1;
  baseFitScale = null;
  gesture = null;
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

const backdropEl = $('panel-backdrop');

function setPanelOpen(open) {
  panelEl.hidden = !open;
  // On mobile the panel is a drawer overlay — show/hide the backdrop.
  backdropEl.hidden = !open;
  $('toggle-files').setAttribute('aria-pressed', open ? 'true' : 'false');
  if (open) listDir(cwd);
}

$('toggle-files').onclick = () => setPanelOpen(panelEl.hidden);
$('close-files').onclick  = () => setPanelOpen(false);
backdropEl.addEventListener('click', () => setPanelOpen(false));

// --- Touch / mobile helpers -------------------------------------------
//
// Feature-detect coarse pointer once; never shown on desktop.
// rfb is module-scoped and re-assigned by startVNC(), so handlers must
// read the variable at event time — never capture it in a closure here.

const hasCoarsePointer = window.matchMedia('(pointer: coarse)').matches;
// Gate gestures on actual touch capability (independent of the media query so
// a touch laptop with a mouse still gets pinch/pan on the canvas).
const hasTouch = 'ontouchstart' in window || navigator.maxTouchPoints > 0;

const touchToolsEl = $('touch-tools');
const touchBarEl   = $('touch-bar');
const fabEl        = $('tb-fab');
const kbdInputEl   = $('keyboard-input');

if (hasCoarsePointer) {
  // Reveal the floating FAB + tool tray (tray starts collapsed behind it).
  touchToolsEl.hidden = false;
}

// --- FAB: expand / collapse the tool tray --------------------------------
function setToolsOpen(open) {
  touchBarEl.classList.toggle('collapsed', !open);
  touchBarEl.setAttribute('aria-hidden', open ? 'false' : 'true');
  fabEl.setAttribute('aria-expanded', open ? 'true' : 'false');
  fabEl.title = open ? 'Hide touch tools' : 'Show touch tools';
}
fabEl.addEventListener('click', () => {
  setToolsOpen(touchBarEl.classList.contains('collapsed'));
});
// Tapping any tool inside the tray collapses it again (keeps the canvas clear).
// The drag-lock toggle is exempt — the user needs the tray to release it.
touchBarEl.addEventListener('click', (e) => {
  const btn = e.target.closest('button');
  if (!btn || btn.id === 'tb-drag') return;
  setToolsOpen(false);
});

// Right-click button — synthesises mousedown+mouseup with button:2 on the
// noVNC canvas. noVNC binds mouse* (not pointer*) listeners on its internal
// canvas (rfb.js:584-591); contextmenu is explicitly early-returned by
// _handleMouse so we omit it. button:2 → bmask=1<<2=4 (RFB right-button).
$('tb-right').addEventListener('click', () => {
  const canvas = screenEl.querySelector('canvas');
  if (!canvas) return;
  const rect = canvas.getBoundingClientRect();
  const cx = rect.left + rect.width / 2;
  const cy = rect.top  + rect.height / 2;
  canvas.dispatchEvent(new MouseEvent('mousedown', {
    bubbles: true, cancelable: true,
    clientX: cx, clientY: cy, button: 2, buttons: 2,
  }));
  canvas.dispatchEvent(new MouseEvent('mouseup', {
    bubbles: true, cancelable: true,
    clientX: cx, clientY: cy, button: 2, buttons: 0,
  }));
});

// Drag-lock button — toggles a persistent left-button-down on the canvas so
// the user can drag without holding the finger. noVNC uses mouse* events only
// (rfb.js:584-586), so we dispatch MouseEvent, not PointerEvent.
let dragLocked = false;
let dragOrigin = null;

$('tb-drag').addEventListener('click', () => {
  const canvas = screenEl.querySelector('canvas');
  if (!canvas) return;
  if (!dragLocked) {
    const rect = canvas.getBoundingClientRect();
    dragOrigin = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    canvas.dispatchEvent(new MouseEvent('mousedown', {
      bubbles: true, cancelable: true,
      clientX: dragOrigin.x, clientY: dragOrigin.y, button: 0, buttons: 1,
    }));
    dragLocked = true;
    $('tb-drag').classList.add('active');
    $('tb-drag').setAttribute('aria-pressed', 'true');
    // Forward subsequent touch moves on the canvas to noVNC as mousemove.
    canvas.addEventListener('touchmove', onDragTouchMove, { passive: false });
  } else {
    // Release the drag lock.
    if (dragOrigin) {
      const canvas2 = screenEl.querySelector('canvas');
      if (canvas2) {
        canvas2.dispatchEvent(new MouseEvent('mouseup', {
          bubbles: true, cancelable: true,
          clientX: dragOrigin.x, clientY: dragOrigin.y, button: 0, buttons: 0,
        }));
      }
    }
    canvas.removeEventListener('touchmove', onDragTouchMove);
    dragLocked = false;
    dragOrigin = null;
    $('tb-drag').classList.remove('active');
    $('tb-drag').setAttribute('aria-pressed', 'false');
  }
});

function onDragTouchMove(e) {
  e.preventDefault();
  const t = e.touches[0];
  if (!t) return;
  const canvas = screenEl.querySelector('canvas');
  if (!canvas) return;
  canvas.dispatchEvent(new MouseEvent('mousemove', {
    bubbles: true, cancelable: true,
    clientX: t.clientX, clientY: t.clientY, button: 0, buttons: 1,
  }));
}

// Keyboard button — focuses the hidden textarea to summon soft keyboard.
$('tb-kbd').addEventListener('click', () => {
  kbdInputEl.focus();
  // Ensure it's on screen for iOS which refuses to show keyboard for
  // elements with zero/tiny bounding box — briefly move it into view.
  kbdInputEl.style.top  = '50%';
  kbdInputEl.style.left = '50%';
  setTimeout(() => {
    kbdInputEl.style.top  = '-200px';
    kbdInputEl.style.left = '-200px';
  }, 300);
});

// Forward keydown events from the hidden textarea into the VNC session.
kbdInputEl.addEventListener('keydown', (e) => {
  if (!rfb) return; // read rfb at event time
  e.preventDefault();
  // Map KeyboardEvent.key to X11 keysym via noVNC's KeyTable when available,
  // otherwise fall back to charCodeAt for printable characters.
  const KeyTable = window.KeyTable;
  let keysym = 0;
  if (KeyTable && KeyTable[e.code]) {
    keysym = KeyTable[e.code];
  } else if (e.key && e.key.length === 1) {
    keysym = e.key.codePointAt(0);
  }
  if (keysym) rfb.sendKey(keysym, e.code, true);
  if (keysym) rfb.sendKey(keysym, e.code, false);
});

// Forward composed text (e.g. CJK IME) as individual codepoints.
kbdInputEl.addEventListener('compositionend', (e) => {
  if (!rfb) return;
  for (const ch of e.data || '') {
    const keysym = ch.codePointAt(0);
    if (keysym) {
      rfb.sendKey(keysym, null, true);
      rfb.sendKey(keysym, null, false);
    }
  }
  kbdInputEl.value = '';
});

// --- Local magnify + pan over the noVNC canvas -------------------------
//
// We drive noVNC's OWN viewport (display.scale / viewportChangePos / clip)
// rather than CSS-transforming the canvas. noVNC maps a click to the
// framebuffer as `cssOffset.x / display.scale + viewportLoc.x` (display.absX),
// dividing by its own stored scale — so as long as we change *its* scale and
// viewport, click coordinates stay correct automatically. A CSS transform
// would desync clicks by the zoom factor.
//
// Zoom model:
//   Z = 1  → "fit": rfb.scaleViewport=true, clipViewport=false (noVNC default).
//            1-finger passes through to noVNC (click / drag / long-press);
//            2-finger drag scrolls the remote (handed to noVNC's gesture
//            handler — we don't intercept the at-fit 2-finger case).
//   Z > 1  → "magnified": rfb.scaleViewport=false, clipViewport=true,
//            display.scale = baseFitScale * Z. 2-finger drag PANS the viewport;
//            pinch keeps the focal framebuffer point under the fingers.
//
// Gesture state machine (capture phase on the stable #screen wrapper, which is
// the parent of noVNC's recreated canvas — capture runs before the canvas's
// bubble-phase gesture listeners, and stopPropagation there prevents noVNC's
// GestureHandler from also acting on the same touches):
//   - 1 finger        → never intercepted; passes through to noVNC.
//   - 2 fingers + Z==1 → pass through to noVNC ONLY if the gesture stays a
//                        2-finger drag (remote scroll). The moment the pinch
//                        distance changes enough we take over and magnify.
//   - 2 fingers + Z>1  → we own it: pinch = zoom about midpoint, drag = pan.

const MAX_ZOOM = 4;
const PINCH_TAKEOVER_RATIO = 0.08; // distance must change >8% before we grab an at-fit pinch

let zoomLevel = 1;        // Z; 1 == fit
let baseFitScale = null;  // noVNC's fit scale captured the moment we leave fit

// Active 2-finger gesture bookkeeping
let gesture = null; // { startDist, startMidX/Y, lastMidX/Y, mode: 'scroll'|'pinch'|null }

function novncDisplay() {
  // rfb is reassigned each startVNC(); reach the internal display defensively.
  return (rfb && rfb._display) ? rfb._display : null;
}

function fitScaleNow() {
  // The scale noVNC currently uses at fit. While scaleViewport=true noVNC keeps
  // display.scale up to date via autoscale, so read it live.
  const d = novncDisplay();
  return d ? d.scale : 1;
}

function applyZoom(z, focusClientX, focusClientY, prevScaleOverride) {
  const d = novncDisplay();
  if (!d) return;
  const canvas = screenEl.querySelector('canvas');
  if (!canvas) return;

  z = Math.max(1, Math.min(MAX_ZOOM, z));

  // Snap back to fit when we reach (or drop below) 1x.
  if (z <= 1) {
    resetToFit();
    return;
  }

  const prevScale = (typeof prevScaleOverride === 'number') ? prevScaleOverride : d.scale;
  const newScale  = baseFitScale * z;

  // Capture the ABSOLUTE framebuffer point under the focal client point BEFORE
  // we change anything (works whether we're at fit or already magnified).
  // noVNC maps screen→fb as fb = cssOffset/scale + viewportLoc; invert it.
  const rect = canvas.getBoundingClientRect();
  const offX = focusClientX - rect.left;
  const offY = focusClientY - rect.top;
  const vpPrev = d._viewportLoc || { x: 0, y: 0 };
  const fbFocusX = offX / prevScale + vpPrev.x;
  const fbFocusY = offY / prevScale + vpPrev.y;

  // Transitioning out of fit: capture the fit scale and switch noVNC into
  // manual clipped-scale mode.
  if (zoomLevel <= 1) {
    baseFitScale = fitScaleNow();
    rfb.scaleViewport = false;
    rfb.clipViewport  = true;
  }

  // THE FILL FIX: size the clip viewport to the CONTAINER (container css px /
  // scale = visible fb px). noVNC then renders canvas = scale * vp = container,
  // so the magnified view fills the whole screen and is cropped to the screen's
  // aspect — instead of scaling the whole-framebuffer rectangle and leaving
  // letterbox dead space. Set the size FIRST so the subsequent _rescale uses it.
  const cw = screenEl.clientWidth;
  const ch = screenEl.clientHeight;
  d.viewportChangeSize(cw / newScale, ch / newScale); // clamps to fb + floors
  d.scale = newScale;                                 // _rescale → canvas css ≈ container

  // Re-place the viewport so the focal fb point lands back under the focal
  // client offset. viewportChangePos takes a framebuffer-unit delta and clamps
  // to the framebuffer edges internally.
  const vpNow = d._viewportLoc || { x: 0, y: 0 };
  const wantX = fbFocusX - offX / newScale;
  const wantY = fbFocusY - offY / newScale;
  d.viewportChangePos(wantX - vpNow.x, wantY - vpNow.y);

  zoomLevel = z;
}

function panBy(deltaCssX, deltaCssY) {
  const d = novncDisplay();
  if (!d || zoomLevel <= 1) return;
  // Content should follow the fingers: dragging right reveals content to the
  // left, i.e. move the viewport origin left → negative framebuffer delta.
  // viewportChangePos clamps to the framebuffer edges internally.
  d.viewportChangePos(-deltaCssX / d.scale, -deltaCssY / d.scale);
}

function resetToFit() {
  zoomLevel = 1;
  baseFitScale = null;
  if (!rfb) return;
  rfb.clipViewport  = false;
  rfb.scaleViewport = true; // noVNC recomputes the fit scale + recenters
}

function dist(t0, t1) {
  return Math.hypot(t0.clientX - t1.clientX, t0.clientY - t1.clientY);
}
function mid(t0, t1) {
  return { x: (t0.clientX + t1.clientX) / 2, y: (t0.clientY + t1.clientY) / 2 };
}

// Intercept in the CAPTURE phase on the stable wrapper. Single-finger touches
// are never stopped (so noVNC keeps clicks/drag/long-press). Two-finger touches
// are intercepted only once we decide to own them.
function onTouchStartCapture(e) {
  if (e.touches.length !== 2) {
    // 1 finger (or 3+): let noVNC handle it. If a 2-finger gesture was in
    // progress, end it.
    gesture = null;
    return;
  }
  const m = mid(e.touches[0], e.touches[1]);
  gesture = {
    startDist: dist(e.touches[0], e.touches[1]),
    lastMidX: m.x, lastMidY: m.y,
    // While zoomed we own the gesture immediately (pan + pinch). At fit we
    // stay 'pending' so a pure 2-finger DRAG can pass through to noVNC as a
    // remote scroll; we only take over once the pinch distance changes enough.
    mode: zoomLevel > 1 ? 'active' : 'pending',
  };
  if (gesture.mode === 'active') {
    e.preventDefault();
    e.stopPropagation();
  }
  // mode 'pending': do NOT stop propagation — noVNC's gesture handler also sees
  // this touchstart and can start its 2-finger drag (remote scroll).
}

function onTouchMoveCapture(e) {
  if (!gesture || e.touches.length !== 2) return;
  const t0 = e.touches[0], t1 = e.touches[1];
  const d = dist(t0, t1);
  const m = mid(t0, t1);

  if (gesture.mode === 'pending') {
    // At fit: decide between remote-scroll (hand to noVNC) and local magnify.
    const ratio = Math.abs(d - gesture.startDist) / (gesture.startDist || 1);
    if (ratio > PINCH_TAKEOVER_RATIO) {
      // Pinch detected → take over for local magnify from here on.
      gesture.mode = 'active';
      // Re-baseline so the first zoom step is smooth from the takeover point.
      gesture.startDist = d;
      gesture.lastMidX = m.x; gesture.lastMidY = m.y;
    } else {
      // Still looks like a pan/scroll — leave it to noVNC.
      gesture.lastMidX = m.x; gesture.lastMidY = m.y;
      return;
    }
  }

  // mode 'active' — we own the gesture.
  e.preventDefault();
  e.stopPropagation();

  // Pan component: midpoint translation.
  const dxMid = m.x - gesture.lastMidX;
  const dyMid = m.y - gesture.lastMidY;
  if (zoomLevel > 1 && (dxMid || dyMid)) {
    panBy(dxMid, dyMid);
  }

  // Pinch component: distance ratio drives the zoom level about the midpoint.
  if (gesture.startDist > 0) {
    const scaleFactor = d / gesture.startDist;
    if (Math.abs(scaleFactor - 1) > 0.001) {
      const prevScale = baseFitScale ? baseFitScale * zoomLevel : fitScaleNow();
      applyZoom(zoomLevel * scaleFactor, m.x, m.y, prevScale);
      gesture.startDist = d; // incremental
    }
  }

  gesture.lastMidX = m.x; gesture.lastMidY = m.y;
}

function onTouchEndCapture(e) {
  // Gesture ends when we drop below 2 active fingers.
  if (e.touches.length < 2) gesture = null;
}

if (hasTouch) {
  // Capture phase + non-passive so preventDefault works for the cases we own.
  screenEl.addEventListener('touchstart', onTouchStartCapture, { capture: true, passive: false });
  screenEl.addEventListener('touchmove',  onTouchMoveCapture,  { capture: true, passive: false });
  screenEl.addEventListener('touchend',   onTouchEndCapture,   { capture: true, passive: false });
  screenEl.addEventListener('touchcancel', onTouchEndCapture,  { capture: true, passive: false });
}

// --- Zoom tool buttons ---------------------------------------------------
function zoomStep(factor) {
  const d = novncDisplay();
  const canvas = screenEl.querySelector('canvas');
  if (!d || !canvas) return;
  const rect = canvas.getBoundingClientRect();
  // Zoom about the canvas centre for button presses.
  const cx = rect.left + rect.width / 2;
  const cy = rect.top  + rect.height / 2;
  const prevScale = baseFitScale ? baseFitScale * zoomLevel : fitScaleNow();
  applyZoom(zoomLevel * factor, cx, cy, prevScale);
}
$('tb-zoom-in').addEventListener('click',  () => zoomStep(1.4));
$('tb-zoom-out').addEventListener('click', () => zoomStep(1 / 1.4));
$('tb-fit').addEventListener('click', () => resetToFit());

// --- boot --------------------------------------------------------------

startVNC();
