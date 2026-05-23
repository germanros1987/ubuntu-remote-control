# Headless GPU server setup

Use the **desktop** install profile when the machine has no X session yet:

```bash
sudo bash packaging/install.sh --profile desktop --gpu nvidia --token YOUR_SECRET
```

## What gets installed

| Package | Purpose |
|---------|---------|
| `xserver-xorg` | Real X server (GPU-backed, not Xvfb) |
| `lightdm` + `ubuntu-desktop-minimal` | Minimal GNOME session |
| `gnome-remote-desktop` | Wayland VNC path when on GNOME |
| `tigervnc-standalone-server` | X11 `x0vncserver` path |
| GPU driver (via `--gpu`) | `nvidia` / `intel` / `amd` / `auto` |

`urc-agent` alone (**minimal** profile) does **not** install any of the above.

## NVIDIA headless notes

1. Install driver: `--gpu nvidia`
2. Many cards need a **HDMI dummy plug** or active display for the GPU to initialize X.
3. Optional `/etc/X11/xorg.conf` snippet:

```
Section "ServerFlags"
    Option "AllowEmptyInitialConfiguration" "true"
EndSection
```

4. Verify after reboot: `nvidia-smi` and `vainfo` inside the logged-in session.

## Intel / AMD

- `--gpu intel` or `--gpu amd`
- Integrated GPUs often work without a dummy plug on recent Ubuntu kernels.

## X11 vs Wayland

| Session | Backend | GPU |
|---------|---------|-----|
| Xorg (`WaylandEnable=false` in gdm) | `x0vncserver` | Apps use GPU on `:0` |
| GNOME Wayland (Ubuntu default) | `gnome-remote-desktop` VNC | PipeWire → Mutter |
| Sway / Hyprland | `wayvnc` | wlroots DRM |

For maximum compatibility with `x0vncserver`, force Xorg:

```ini
# /etc/gdm3/custom.conf
WaylandEnable=false
```

## Troubleshooting

| Symptom | Check |
|---------|-------|
| Agent exits: no graphical session | `loginctl list-sessions`, log in locally or use `--profile desktop` |
| Black screen over VNC (GNOME) | `journalctl --user -u gnome-remote-desktop`; try Xorg |
| `vainfo` fails in VNC but works over SSH | You may be on a virtual Xvfb; use real Xorg + GPU driver |
| VNC works locally but not remotely | Coordinator token, firewall, `urc-coordinator` on VPS |

## File transfer

With agent running, files API is on `127.0.0.1:15901` (forward through coordinator or Tailscale):

```bash
curl http://127.0.0.1:15901/api/list/
curl -F file=@./myfile.bin http://127.0.0.1:15901/api/upload/home/user/incoming/myfile.bin
```

## Tailscale direct path

```toml
[tailscale]
enabled = true
prefer_direct = true
```

Client will show a hint to connect to `100.x.x.x:15900` (TLS) when the host registers its Tailscale IP.
