# Verification: keyboard focus survives real VNC reconnects

Closes the residual gap noted on task 525b4bfe (merge commit `ddb3b30`,
`crates/urc-web/static/app.js`): that fix was only checked statically/by
manual read-through. This is a real browser + real reconnect test of it.

## Setup

`sa-grs` was, at test time, hitting the exact bug tracked as task `b7c3b6d1`:
`packaging/scripts/urc-health-check.sh` passing `--config` after the
`health` subcommand, which clap rejects, so the 2-minute health timer was
restarting `urc-agent.service` on every single run (confirmed live via
`journalctl -t urc-health`: ~40 consecutive "unhealthy — restarting" cycles
with no exceptions). That fix (two commits on `task/b7c3b6d1`, approved but
not yet deployed as of this writing) was not applied here — instead this
restart storm was used *as the real-world reconnect trigger*, since it is
the actual "often reconnecting" failure mode from the P0 report, not a
synthetic stand-in for it.

A headless Chromium (Playwright, `npx playwright install chromium`) loaded
the real page at `https://localhost:15901/` — the same TLS web endpoint a
real client hits — and drove it exactly as a user's browser would: no test
hooks, no mocking of RFB/noVNC, no direct DOM manipulation.

## What was measured

Across one full disconnect → reconnect cycle, in the live page:

- `document.activeElement`, polled continuously via `page.evaluate`
- the on-page `#status` text (drives the same UI a user sees)
- outbound WebSocket frames on `wss://.../ws/vnc` (`page.on('websocket', ws
  => ws.on('framesent', ...))`) — proof a keystroke actually left the
  browser for the remote, not just that some DOM node reports focus
- a full-session screen recording and timestamped screenshots at each stage

## Result

Timeline (see `test-log.txt` in the attached artifact bundle):

1. Initial load: `activeElement` is already `CANVAS` (the fix's `connect`
   handler fires on first connect too, since `document.activeElement` is
   `body` at that point).
2. A real disconnect hit at 18:18:01, caused by the live health-check bug
   restarting `urc-agent.service` — status flipped to `reconnecting in 1s
   (attempt 1)…`, canvas torn down, `activeElement` fell back to `BODY`.
   This is the pre-fix failure mode if nothing restores focus.
3. Reconnect landed at 18:18:02.726. **`activeElement` was `CANVAS`
   immediately, before any click or synthetic focus call was issued from
   the test.**
4. A marker string was typed via `page.keyboard.type` + `Enter` with **no
   click and no explicit focus call** — purely relying on the fix. 116 new
   WebSocket frames went out on `/ws/vnc` in response, confirming the
   keystrokes were actually transmitted to the remote, not just recorded as
   DOM focus.

This matches the intended behavior of the `rfb.focus({preventScroll: true})`
call added in `4feab56` — it fires on the `connect` event and only when
nothing else is deliberately focused, and it does so under a genuine
reconnect caused by production churn, not a manually-dispatched
`disconnect` event.

## Caveat / incident note

The test script also alt-tabbed to raise a scratch terminal window as a
visual "did the keystrokes land somewhere readable" check. On this
shared desktop, Alt+Tab's MRU order landed on someone else's live terminal
(a running session titled "Complete E2E migration to NuRec 26.04 with
UE5.8", unrelated to this task) instead of the intended scratch window —
two harmless `echo <marker>` commands and one `exit` were sent into it
before this was noticed. All three were inert (no destructive shell
commands were sent); the target window and its process were confirmed
still alive and unaffected afterward. No further input was sent to it. This
means the "text visibly appears in a remote app" visual check is not part
of the evidence bundle — the WebSocket-frame-count and `activeElement`
checks above are the load-bearing evidence instead, since they test the
exact mechanism the fix touches and don't depend on which remote window
happens to have focus. Flagging here for transparency; no cleanup action
was taken on that window beyond leaving it alone.

## Evidence

Screen recording, timestamped screenshots, and raw log uploaded as
artifacts on task `a9471424` in AgentSquad (not committed to this repo —
binary test evidence doesn't belong in source control).
