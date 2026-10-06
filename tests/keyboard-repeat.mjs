// Run: node --experimental-vm-modules --test tests/keyboard-repeat.mjs
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

// Load the real vendored keyboard class with browser dependencies stubbed;
// exercising its output does not require a DOM or a running VNC server.
const source = await readFile(new URL('../crates/urc-web/static/novnc/core/input/keyboard.js', import.meta.url), 'utf8');
const module = new vm.SourceTextModule(source);
await module.link((name) => {
    const values = name.endsWith('logging.js') ? { Debug() {} }
        : name.endsWith('events.js') ? { stopEvent() {} }
        : name.endsWith('keysym.js') ? { default: {} } : {};
    return new vm.SyntheticModule(Object.keys(values), function () {
        for (const [key, value] of Object.entries(values)) this.setExport(key, value);
    });
});
await module.evaluate();
const Keyboard = module.namespace.default;

test('client repeats become fresh presses without releasing held modifiers', () => {
    const keyboard = new Keyboard(null);
    const events = [];
    keyboard.onkeyevent = (key, code, down) => events.push([key, code, down]);
    keyboard._sendKeyEvent(0xffe1, 'ShiftLeft', true);
    keyboard._sendKeyEvent(65, 'KeyA', true);
    keyboard._sendKeyEvent(65, 'KeyA', true);
    keyboard._sendKeyEvent(65, 'KeyA', true);
    keyboard._allKeysUp();
    assert.deepEqual(events, [
        [0xffe1, 'ShiftLeft', true], [65, 'KeyA', true],
        [65, 'KeyA', false], [65, 'KeyA', true],
        [65, 'KeyA', false], [65, 'KeyA', true],
        [0xffe1, 'ShiftLeft', false], [65, 'KeyA', false],
    ]);
});
