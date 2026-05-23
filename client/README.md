# URC native client (future Tauri UI)

The production CLI client is **`urc-client`** (Rust), which:

- Registers with the coordinator
- Opens a local VNC port via relay
- Launches `vncviewer` / TigerVNC

## macOS

```bash
brew install tigervnc-viewer
urc-client --coordinator ws://vps:21150/ws/client --token SECRET connect hostname
```

**Clipboard:** use TigerVNC viewer (extended clipboard).  
**Cmd → Super:** set `URC_MAC_CMD_TO_SUPER=1` (default in `urc-client connect`).

## Linux

```bash
sudo apt install tigervnc-viewer
urc-client connect hostname
```

A Tauri GUI (connection wizard, file drag-drop) is planned; the protocol and relay are shared with the CLI.
