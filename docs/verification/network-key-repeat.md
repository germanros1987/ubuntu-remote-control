# Network-sensitive keyboard repetition

The installed TigerVNC 1.13.1 X11 backend injects VNC key-down and key-up events
using XTEST. If a key-up arrives after Ubuntu's repeat delay, X11 generates extra
characters locally; the client need not have sent duplicate presses.

The isolated XTEST test reproduced 15 KeyPress events from a single press held
for 1.2 seconds before release, and one with the repeat guard active. This
reproduces delayed-release behavior; it is not a macOS Screen Sharing test or a
measurement of a particular user's network.

## Changes and tradeoffs

- The X11 backend starts a desktop-user helper that disables server repeat and
  holds a lifetime pipe. It snapshots the master keyboard and attached slaves.
  EOF (including an agent crash) or SIGTERM/SIGINT explicitly restores their
  original repeat settings, without changing repeat timing, layout or modifiers.
- XKB auto-reset controls provide additional recovery if the helper is killed.
  Explicit restoration is still necessary: the tested X server restores XKB
  flags on connection loss but does not fully restore keyboard feedback. A hard
  SIGKILL of the helper can therefore require `xset r on` from the desktop user's
  X11 session, or a backend restart and normal stop. Normal systemd stop/restart
  sends SIGTERM and runs explicit restoration.
- Protection lasts for the X11 backend lifetime, even with no viewer attached.
  Physical-keyboard hold-to-repeat is disabled during that time. Native viewers
  sending repeated down events without releases may also lose hold-to-repeat;
  macOS Screen Sharing needs manual validation. Set top-level
  `x11_server_key_repeat = true` to retain host repeat and opt out of protection.
- The bundled browser keyboard turns each client repeat into a release/press
  pair for that key, preserving held modifiers and normal focus-loss cleanup.
- TLS forwarding uses TCP_NODELAY and `copy_bidirectional`, which flushes traffic
  and propagates EOF. Previously, joined copy loops could leave VNC connected
  after a viewer closed, preventing VNC from releasing held keys. Web and relay
  VNC sockets also use TCP_NODELAY; the relay ends both halves on disconnect.

The guard uses the [XKB auto-reset mechanism](https://www.x.org/archive/X11R7.7/doc/libX11/XKB/xkblib.html)
and explicit SetControls requests for full restoration on the tested X server.

## Checks

```bash
cargo build -p urc-agent
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
node --experimental-vm-modules --test tests/keyboard-repeat.mjs
python3 tests/x11-key-repeat.py
```

Local results: workspace tests, the isolated X11 test (debug and release), the
browser test, formatting, and Clippy for `urc-agent`, `urc-client`, and `urc-web`
pass. Workspace-wide Clippy is blocked by an existing
`clippy::overly_complex_bool_expr` in `crates/urc-coordinator/src/state.rs:42`.

The Python test needs `tigervnc-standalone-server`, `x11-xserver-utils`, and
`libxtst6`. It starts a private X server with no VNC listening port and never
injects keys into the active desktop. It checks the delayed-release reproduction,
separate intentional strokes, EOF and SIGTERM restoration, and preservation of
an initially disabled repeat setting. The Rust regression sends real RFB key
packets through TLS to a fake VNC socket and verifies bytes and disconnect EOF.
The JavaScript test checks intentional repeats, held Shift, and focus-loss cleanup.

## Apply a locally built fix

On each Ubuntu host, from this checkout:

```bash
cargo build --release -p urc-agent
sudo install -m 755 target/release/urc-agent /usr/local/bin/urc-agent
sudo systemctl restart urc-agent
```

Restarting disconnects current viewers. Reconnect afterwards; browser users
should reload the page to load the updated keyboard code. Once merged and
released, the normal agent installer distributes the same changes. Mac clients
must be rebuilt/updated on macOS to pick up the client-side tunnel improvements.

Manually validate native viewers with normal typing, quick repeated letters,
Shift/Ctrl/Alt shortcuts, held Backspace/arrows, and closing a viewer while a key
is down. Test a slow connection as well as a fast one. The automated tests do
not establish compatibility with every native viewer or Wayland backend.
