# Ubuntu Remote Control (URC)

Remote your Ubuntu machines through a relay — your **real** GPU desktop, encrypted, reachable behind NAT.

## Install (curl only — no git)

**Interactive** (recommended — one question per machine):

```bash
curl -fsSL https://YOUR_SERVER/install | sudo bash
```

Set `URC_GITHUB=youruser/ubuntu-remote-control` if the script should pull source from GitHub (default). Or host your own tarball/binaries (see below).

**What you pick:**

| Prompt | Machine |
|--------|---------|
| 1 | Ubuntu PC you remote **into** |
| 2 | VPS **relay** (open TCP 21150) |
| 3 | Laptop you **connect from** |

Then connect:

```bash
urc connect my-pc-name
```

Credentials land in `/etc/urc/credentials` on each box.

### Three commands (non-interactive)

```bash
# VPS
curl -fsSL https://YOUR_SERVER/install | sudo bash -s -- --role coordinator -y

# Home PC
curl -fsSL https://YOUR_SERVER/install | sudo bash -s -- --role agent \
  --coordinator-url ws://VPS_IP:21150/ws/agent \
  --token YOUR_TOKEN -y

# Laptop
curl -fsSL https://YOUR_SERVER/install | sudo bash -s -- --role client \
  --coordinator-url ws://VPS_IP:21150/ws/client \
  --token YOUR_TOKEN --host-id my-pc -y

urc connect my-pc
```

### Host your own install URL

From a machine with Rust:

```bash
./packaging/mk-dist.sh
# Upload dist/install, dist/urc-source.tar.gz, and/or dist/linux-x86_64/ to HTTPS
```

**Fast install (prebuilt binaries, no rustc on target):**

```bash
curl -fsSL https://YOUR_SERVER/install | sudo \
  URC_BINARIES_URL=https://YOUR_SERVER/linux-x86_64 \
  URC_RAW_URL=https://YOUR_SERVER/raw/main \
  bash
```

**Tarball only:**

```bash
curl -fsSL https://YOUR_SERVER/install | sudo \
  URC_SOURCE_TARBALL=https://YOUR_SERVER/urc-source.tar.gz bash
```

### Local test (before you host it)

```bash
./packaging/mk-dist.sh
curl -fsSL file://$PWD/dist/install | sudo \
  URC_SOURCE_TARBALL=file://$PWD/dist/urc-source.tar.gz bash
```

Or from checkout without curl:

```bash
sudo ./install
```

## Reliability

Auto-restart on crash, boot, login, and every 10 minutes. See [docs/service-health.md](docs/service-health.md).

## License

MIT (application). VNC backends are GPL system packages.
