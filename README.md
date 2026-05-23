# Ubuntu Remote Control (URC)

Remote your Ubuntu PCs over **Tailscale** — real GPU desktop, encrypted, no VPS required.

## 1-click install (recommended)

Use the **same Tailscale account** on every machine.

### PC you control (Ubuntu desktop, e.g. sa-grs)

```bash
curl -fsSL https://github.com/germanros1987/ubuntu-remote-control/releases/latest/download/install | \
  sudo bash -s -- --role agent -y
```

First run builds from source (~3–5 min). Sign in to Tailscale when prompted. Stay logged into the graphical desktop on that PC.

### Mac you connect from

```bash
curl -fsSL https://github.com/germanros1987/ubuntu-remote-control/releases/latest/download/install | \
  sudo bash -s -- --role client -y
```

Installs `urc` + Tailscale. Uses built-in **Screen Sharing** (no extra VNC app).

### Connect

```bash
urc hosts
urc connect sa-grs
```

Keep the terminal open while you use the remote desktop. Press **Ctrl+C** when done.

---

## If something fails

**On the PC** (re-run the same curl command — it is safe):

```bash
curl -fsSL https://github.com/germanros1987/ubuntu-remote-control/releases/latest/download/install | \
  sudo bash -s -- --role agent -y
```

Check:

```bash
systemctl status urc-agent
ss -tlnp | grep 15900
```

**On the Mac** (re-run client install, then connect):

```bash
curl -fsSL https://github.com/germanros1987/ubuntu-remote-control/releases/latest/download/install | \
  sudo bash -s -- --role client -y
```

---

## Non-interactive / Tailscale auth key

```bash
URC_TAILSCALE_AUTH_KEY=tskey-auth-... \
  curl -fsSL https://github.com/germanros1987/ubuntu-remote-control/releases/latest/download/install | \
  sudo bash -s -- --role agent -y
```

## What happens under the hood

| Step | What URC does |
|------|----------------|
| Install on PC | Agent + TigerVNC + Tailscale; TLS VNC on tailnet port **15900** |
| Install on Mac | Client + Tailscale; tunnels to PC, opens Screen Sharing |
| `urc hosts` | Lists machines from `tailscale status` |
| `urc connect NAME` | TLS tunnel → plain VNC locally → Screen Sharing |

Optional **VPS relay**: pass `--coordinator-url` during install (advanced).

## Local development

```bash
sudo ./install --role agent -y    # on Ubuntu PC
sudo ./install --role client -y   # on Mac (from repo checkout)
```

## Reliability

Auto-restart on crash, boot, login, and health timers. See [docs/service-health.md](docs/service-health.md).

## License

[MIT](LICENSE) — application code. VNC backends are GPL system packages from Ubuntu.
