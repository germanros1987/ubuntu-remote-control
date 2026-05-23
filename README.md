# Ubuntu Remote Control (URC)

Remote your Ubuntu PCs over **Tailscale** — real GPU desktop, encrypted, no VPS required.

## Turn-key setup (two installs)

Use the **same Tailscale account** on every machine.

### 1. Each PC you want to control

```bash
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | sudo bash
# When prompted, choose: 1) PC I remote INTO
# Sign in to Tailscale when prompted
```

Or pass the role directly (no menu):

```bash
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | \
  sudo bash -s -- --role agent
```

### 2. Your laptop (Linux or macOS)

**Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | sudo bash
# Choose: 2) Laptop I connect FROM
```

**macOS** (client only — same Tailscale account):
```bash
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | sudo bash
# Detects macOS and installs client + Tailscale automatically
# VNC viewer: brew install --cask tigervnc-viewer
```

```bash
# Non-interactive (Linux or Mac):
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | \
  sudo bash -s -- --role client
```

### 3. Connect

```bash
urc hosts              # all machines on your tailnet
urc connect my-pc      # open remote desktop
```

That is it — no coordinator URL, no tokens to copy, no host IDs to configure by hand. Machine names come from Tailscale (defaults to your PC hostname).

## What happens under the hood

| Step | What URC does |
|------|----------------|
| Install on PC | Agent + TigerVNC + Tailscale; agent listens on TLS port 15900 on the tailnet |
| Install on laptop (Linux or Mac) | Client + Tailscale |
| `urc hosts` | Reads your tailnet from `tailscale status` |
| `urc connect NAME` | Resolves NAME → Tailscale IP → encrypted VNC |

Optional **VPS relay** is still available for advanced setups (`--coordinator-url` during install). Most users only need Tailscale.

## Non-interactive / auth keys

```bash
# PC (unattended Tailscale login)
URC_TAILSCALE_AUTH_KEY=tskey-auth-... \
  curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | \
  sudo bash -s -- --role agent -y

# Laptop
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | \
  sudo bash -s -- --role client -y
```

## Self-hosted install mirror (optional)

By default the installer pulls source from this repo on GitHub. To host your own copy (e.g. prebuilt binaries):

```bash
./packaging/mk-dist.sh
# Upload dist/ to HTTPS, then see packaging/mk-dist.sh for URC_BINARIES_URL examples
```

Repository: [github.com/germanros1987/ubuntu-remote-control](https://github.com/germanros1987/ubuntu-remote-control)

Local test from checkout:

```bash
sudo ./install
```

## Reliability

Auto-restart on crash, boot, login, and every 10 minutes. See [docs/service-health.md](docs/service-health.md).

## License

[MIT](LICENSE) — application code. VNC backends are GPL system packages from Ubuntu.
