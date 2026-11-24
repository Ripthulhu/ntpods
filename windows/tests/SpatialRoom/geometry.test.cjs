"use strict";
const test = require("node:test"), assert = require("node:assert/strict");
const G = require("../../winui/NTPods.WinUI/Assets/SpatialRoom/geometry.js");
const approx = (a, b, tol = 1e-6) => assert.ok(Math.abs(a - b) <= tol, `${a} ≠ ${b}`);
const vec = (a, b) => b.forEach((x, i) => approx(a[i], x));
const front = [0, 0, -1], right = [1, 0, 0];
test("effective speaker angles follow Windows positive-right, positive-up bearings", () => {
  vec(G.direction(0, 0), front);
  vec(G.direction(90, 0), right);
  vec(G.direction(0, 90), [0, 1, 0]);
  for (const [az, el] of [[-135, -24], [30, 48], [90, 0], [0, 65]]) {
    const angles = G.angles(G.direction(az, el));
    approx(angles.az, az);
    approx(angles.el, el);
  }
});
test("physical head rotation has the same three signs as the renderer", () => {
  vec(G.transform(G.headRotation(90, 0, 0), front, 0), [1, 0, 0, 0]);
  vec(G.transform(G.headRotation(0, 90, 0), front, 0), [0, 1, 0, 0]);
  vec(G.transform(G.headRotation(0, 0, 90), right, 0), [0, -1, 0, 0]);
  vec(G.transform(G.headRotation(90, 30, 0), front, 0), [Math.sqrt(.75), .5, 0, 0]);
});
test("normalized cabinets point at the listener at all elevations, including poles", () => {
  for (const [az, el] of [[0, 0], [135, -45], [20, 90], [-70, -90]]) {
    const c = G.cabinet({bit: 0, az, el});
    vec(c.front, G.unit(G.scale(c.origin, -1)));
    approx(G.dot(c.up, c.front), 0);
    approx(G.dot(c.right, c.up), 0);
    vec(G.cross(c.right, c.up), c.front);
    vec(G.transform(c.matrix, [0, 0, 0]), [...c.origin, 1]);
  }
});
test("screen projection agrees with click picking through orbit, zoom and resize", () => {
  for (const [w, h] of [[550, 340], [350, 490], [1200, 330]])
    for (const [az, el, zoom] of [[18, 32, 1], [-65, 74, 1.4], [135, 20, .7]]) {
      const c = G.camera({az, el, zoom}, w, h), center = G.project(c, [0, 0, 0]);
      approx(center.x, w / 2);
      approx(center.y, h / 2);
      for (const speaker
               of [{bit: 0, az: -30, el: 0}, {bit: 9, az: -100, el: 50}, {bit: 3, az: 0, el: 0}]) {
        const b = G.cabinet(speaker), xy = G.project(c, b.origin);
        const ray = G.ray(c, xy.x, xy.y);
        assert.equal(G.pick(ray, [b]), b.bit);
        vec(ray.direction, G.unit(G.sub(b.origin, c.eye)));
      }
    }
});
test("overlapping cabinets resolve to their nearest hit; air doesn't select", () => {
  const base = G.cabinet({bit: 2, az: 0, el: 0}), near = {...base, bit: 1, origin: [0, 0, 2]};
  const ray = {origin: [0, 0, 8], direction: [0, 0, -1]};
  assert.equal(G.pick(ray, [base, near]), 1);
  assert.equal(G.pick(ray, [near, base]), 1);
  assert.equal(G.pick({origin: [8, 0, 8], direction: [0, 0, -1]}, [base]), null);
});
test("each cabinet in the 300% 7.1 room is visible and selectable in the default view", () => {
  const speakers = [[0, -60], [1, 60], [2, 0], [4, -120], [5, 120], [9, -90], [10, 90]]
    .map(([bit, az]) => G.cabinet({bit, az, el: 0}));
  for (const [w, h] of [[320, 300], [640, 340]]) {
    const camera = G.camera(G.defaultView(), w, h);
    for (const speaker of speakers) {
      const p = G.project(camera, speaker.origin);
      assert.ok(p.x > 0 && p.x < w && p.y > 0 && p.y < h);
      assert.equal(G.pick(G.ray(camera, p.x, p.y), speakers), speaker.bit);
    }
  }
});
test("dragging a rear or front cabinet preserves its hemisphere when both roots exist", () => {
  const c = G.camera(G.defaultView(), 640, 350);
  for (const [az, el] of [[0, 0], [30, 0], [-150, 20], [120, 45]]) {
    const original = G.scale(G.direction(az, el), G.radius), screen = G.project(c, original);
    const result = G.spherePoint(G.ray(c, screen.x, screen.y), original);
    assert.ok(result);
    vec(result, original);
  }
  assert.equal(G.spherePoint({origin: [10, 10, 10], direction: [1, 0, 0]}, front), null);
});
