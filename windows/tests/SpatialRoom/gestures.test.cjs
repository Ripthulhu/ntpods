"use strict";
const test = require("node:test"), assert = require("node:assert/strict");
const {readFileSync} = require("node:fs"), {runInNewContext} = require("node:vm");
const G = require("../../winui/NTPods.WinUI/Assets/SpatialRoom/geometry.js");
const source = readFileSync(require.resolve("../../winui/NTPods.WinUI/Assets/SpatialRoom/room.js"), "utf8");

function room(width, positions) {
  const elements = {}, tags = [], messages = [];
  let draw, lastFrame;
  function element() {
    return Object.assign(new EventTarget(), {
      dataset: {}, style: {setProperty() {}}, focus() {}, setPointerCapture() {}, remove() {},
      append(tag) { tags.push(tag); },
      getBoundingClientRect() {
        const x = parseFloat(this.style.left) || 0, y = parseFloat(this.style.top) || 0;
        return this.className === "speaker-label"
          ? {left: x - 10, right: x + 10, top: y - 9.5, bottom: y + 9.5}
          : {left: 0, top: 0};
      }
    });
  }
  const document = {
    documentElement: element(), createElement: element,
    getElementById(id) { return elements[id] ??= element(); }
  };
  const window = new EventTarget(), bridge = new EventTarget();
  window.chrome = {webview: bridge};
  bridge.postMessage = message => messages.push(structuredClone(message));
  const viewport = document.getElementById("viewport");
  viewport.clientWidth = width;
  viewport.clientHeight = 340;
  runInNewContext(source, {
    window, document, RoomGeometry: G, devicePixelRatio: 1, performance,
    ResizeObserver: class { observe() {} },
    requestAnimationFrame(callback) { draw = callback; return 1; },
    createStudio: () => ({frame(state, cam, boxes) { lastFrame = {state, cam, boxes}; }})
  });
  assert.deepEqual(messages.splice(0), [{type: "ready"}]);
  const speakers = positions.map(([bit, az]) => ({bit, az, el: 0}));
  function snapshot(selectedMask, updated = speakers) {
    const data = {type: "room", speakers: updated, selectedMask};
    bridge.dispatchEvent(Object.assign(new Event("message"), {data}));
    draw();
  }
  snapshot(0);
  const tag = bit => tags.find(tag => tag.dataset.bit === String(bit));
  return {
    messages, speakers, snapshot, tag,
    point(bit) {
      return G.project(lastFrame.cam, lastFrame.boxes.find(b => b.bit === bit).origin);
    },
    labelPoint(bit) {
      const label = tag(bit);
      assert.equal(label.hidden, false);
      return {x: parseFloat(label.style.left), y: parseFloat(label.style.top)};
    },
    state() { return lastFrame.state; },
    pointer(type, p, ctrlKey = false, id = 1) {
      elements.stage.dispatchEvent(Object.assign(new Event(type), {
        button: 0, pointerId: id, clientX: p.x, clientY: p.y, ctrlKey
      }));
    }
  };
}

test("plain and Ctrl-clicks distinguish overlapping 500% room channels by their visible labels", () => {
  const positions = [[0,-90], [2,0], [1,90], [9,-90], [10,90], [4,-120], [5,120]];
  for (const width of [320, 640]) {
    const scene = room(width, positions);
    for (const [bit, toggle] of [[1, false], [10, true], [1, true], [0, false], [9, true]]) {
      const p = scene.labelPoint(bit);
      scene.pointer("pointerdown", p, toggle);
      scene.pointer("pointerup", p, toggle);
      assert.deepEqual(scene.messages.splice(0), [{type: "select", bit, toggle, edit: true}]);
    }
    scene.snapshot((1 << 1) | (1 << 10));
    for (const s of scene.state().speakers) {
      assert.equal(s.selected, s.bit === 1 || s.bit === 10);
      assert.equal(scene.tag(s.bit).dataset.selected, String(s.selected));
    }
    scene.snapshot(0);
    assert.ok(scene.state().speakers.every(s => !s.selected));
  }
});

test("a cabinet drag selects once and moves only its anchor through asynchronous room frames", () => {
  const scene = room(640, [[0,-30], [1,30], [2,0]]);
  scene.snapshot(3);
  const p = scene.point(1), q = {x: p.x + 10, y: p.y - 5};
  scene.pointer("pointerdown", p, true);
  scene.pointer("pointermove", {x: p.x + 2, y: p.y}, true);
  assert.deepEqual(scene.messages, []);
  scene.pointer("pointermove", q, true);
  assert.deepEqual(scene.messages[0], {type: "select", bit: 1, drag: true});
  const move = scene.messages[1];
  assert.equal(move.type, "move");
  assert.equal(move.bit, 1);
  // The native layout constrains and positions all members of the group.
  scene.snapshot(3, scene.speakers.map(s => ({...s, el: 3})));
  assert.equal(scene.state().speakers[1].el, 3);
  scene.pointer("pointerup", q, true);
  scene.pointer("lostpointercapture", q, true);
  assert.deepEqual(scene.messages, [{type: "select", bit: 1, drag: true}, move, move]);
});

test("cancel, pinch, empty-space orbit and layout removal do not edit the selection", () => {
  const scene = room(640, [[0,-30], [1,30]]);
  const p = scene.point(0), q = scene.point(1);
  scene.pointer("pointerdown", p, true);
  scene.pointer("pointercancel", p, true);
  scene.pointer("pointerdown", p, true);
  scene.pointer("pointerdown", q, false, 2);
  scene.pointer("pointermove", {x: q.x + 5, y: q.y}, false, 2);
  scene.pointer("pointerup", q, false, 2);
  scene.pointer("pointerup", p, true);
  scene.pointer("pointerdown", {x: 0, y: 0});
  scene.pointer("pointermove", {x: 12, y: 8});
  scene.pointer("pointerup", {x: 12, y: 8});
  scene.pointer("pointerdown", p, true);
  scene.snapshot(0, []);
  scene.pointer("pointerup", p, true);
  assert.deepEqual(scene.messages, []);
});
