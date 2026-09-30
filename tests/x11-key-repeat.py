#!/usr/bin/env python3
"""Isolated XTEST regression: python3 tests/x11-key-repeat.py [agent-binary].

Requires Xtigervnc, xset, libX11 and libXtst. Starts a private X server and never
injects events into the user's display. XTEST is the API used by X0tigervnc.
"""
import ctypes as C
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time


def bind(lib, name, args, result=C.c_int):
    fn = getattr(lib, name)
    fn.argtypes, fn.restype = args, result
    return fn


agent = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/urc-agent").resolve()
x = C.CDLL("libX11.so.6")
t = C.CDLL("libXtst.so.6")
P, I, U = C.c_void_p, C.c_int, C.c_ulong
bind(x, "XOpenDisplay", [C.c_char_p], P)
bind(x, "XCloseDisplay", [P])
bind(x, "XDefaultRootWindow", [P], U)
bind(x, "XCreateSimpleWindow", [P, U, I, I, C.c_uint, C.c_uint, C.c_uint, U, U], U)
bind(x, "XSelectInput", [P, U, C.c_long])
bind(x, "XMapWindow", [P, U])
bind(x, "XSetInputFocus", [P, U, I, U])
bind(x, "XSync", [P, I])
bind(x, "XPending", [P])
bind(x, "XNextEvent", [P, P])
bind(t, "XTestFakeKeyEvent", [P, C.c_uint, I, U])

with tempfile.TemporaryFile() as log:
    read_fd, write_fd = os.pipe()
    server = subprocess.Popen([
        "Xtigervnc", "-displayfd", str(write_fd), "-localhost", "-rfbport", "-1",
        "-SecurityTypes", "None", "-geometry", "640x480", "-depth", "24", "-nolisten", "tcp",
    ], pass_fds=(write_fd,), stdout=log, stderr=log)
    os.close(write_fd)
    display = None
    guard = None
    try:
        assert select.select([read_fd], [], [], 10)[0], "private X server did not start"
        number = os.read(read_fd, 64).decode().strip()
        assert number.isdigit(), "private X server failed"
        env = {**os.environ, "DISPLAY": f":{number}"}
        display = x.XOpenDisplay(env["DISPLAY"].encode())
        assert display
        window = x.XCreateSimpleWindow(display, x.XDefaultRootWindow(display), 0, 0, 100, 100, 0, 0, 0)
        x.XSelectInput(display, window, 3)  # KeyPressMask | KeyReleaseMask
        x.XMapWindow(display, window)
        x.XSetInputFocus(display, window, 1, 0)
        x.XSync(display, 0)

        def drain():
            presses = 0
            event = (C.c_long * 24)()
            while x.XPending(display):
                x.XNextEvent(display, event)
                presses += (event[0] & 0xffffffff) == 2
            return presses

        def key(down):
            t.XTestFakeKeyEvent(display, 38, down, 0)  # A on the private default keymap
            x.XSync(display, 0)

        def delayed_release():
            drain()
            key(True)
            time.sleep(1.2)
            key(False)
            return drain()

        def start_guard():
            proc = subprocess.Popen([str(agent), "x11-repeat-guard"], env=env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            assert select.select([proc.stdout], [], [], 5)[0], "guard did not become ready"
            assert proc.stdout.readline() == b"ready\n"
            return proc

        def repeat_setting():
            output = subprocess.check_output(["xset", "q"], env=env, text=True)
            return [line.strip() for line in output.splitlines() if "auto repeat" in line]

        subprocess.run(["xset", "r", "on"], env=env, check=True)
        original = repeat_setting()
        baseline = delayed_release()
        assert baseline > 1, baseline
        print(f"Without protection: {baseline} characters for one delayed release")
        guard = start_guard()
        assert delayed_release() == 1
        for _ in range(4):
            key(True)
            key(False)
        assert drain() == 4, "separate intentional strokes must survive"
        # EOF models graceful stop AND an agent crash closing the lifetime pipe.
        guard.stdin.close()
        assert guard.wait(timeout=5) == 0
        assert repeat_setting() == original
        assert delayed_release() > 1, "restore actual repeat, not just the XKB flag"
        guard = start_guard()
        guard.terminate()
        assert guard.wait(timeout=5) == 0
        assert repeat_setting() == original
        assert delayed_release() > 1
        subprocess.run(["xset", "r", "off"], env=env, check=True)
        original = repeat_setting()
        guard = start_guard()
        guard.stdin.close()
        assert guard.wait(timeout=5) == 0
        assert repeat_setting() == original
        assert delayed_release() == 1, "originally disabled repeat must stay disabled"
        print("PASS: delayed release, intentional strokes, EOF, SIGTERM, original on/off restoration")
    finally:
        if guard and guard.poll() is None:
            guard.terminate()
            guard.wait(timeout=5)
        if display:
            x.XCloseDisplay(display)
        os.close(read_fd)
        server.terminate()
        server.wait(timeout=5)
