// Scene axes use +X right, +Y up, -Z toward the screen. Matrix storage is column-major.
"use strict";
const RoomGeometry = (() => {
  const rad = Math.PI / 180;
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
  const add = (a, b) => a.map((v, i) => v + b[i]);
  const sub = (a, b) => a.map((v, i) => v - b[i]);
  const scale = (a, s) => a.map(v => v * s);
  const dot = (a, b) => a.reduce((s, v, i) => s + v * b[i], 0);
  const cross =
      (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const unit = a => scale(a, 1 / (Math.hypot(...a) || 1));
  const axes =
      (right, up, front, origin = [0, 0, 0]) => [...right, 0, ...up, 0, ...front, 0, ...origin, 1];
  const identity = () => axes([1, 0, 0], [0, 1, 0], [0, 0, 1]);
  const displacement = (x, y, z) => axes([1, 0, 0], [0, 1, 0], [0, 0, 1], [x, y, z]);
  const size = (x, y, z) => axes([x, 0, 0], [0, y, 0], [0, 0, z]);
  function multiply(a, b) {
    const out = Array(16);
    for (let c = 0; c < 4; c++)
      for (let r = 0; r < 4; r++)
        out[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2]
            + a[12 + r] * b[c * 4 + 3];
    return out;
  }
  const transform = (m, p, w = 1) => [0, 1, 2, 3].map(
      i => m[i] * p[0] + m[i + 4] * p[1] + m[i + 8] * p[2] + m[i + 12] * w);
  const cosSin = deg => [Math.cos(deg * rad), Math.sin(deg * rad)];
  function rotX(deg) {
    const [c, s] = cosSin(deg);
    return axes([1, 0, 0], [0, c, s], [0, -s, c]);
  }
  function rotY(deg) {
    const [c, s] = cosSin(deg);
    return axes([c, 0, -s], [0, 1, 0], [s, 0, c]);
  }
  function rotZ(deg) {
    const [c, s] = cosSin(deg);
    return axes([c, s, 0], [-s, c, 0], [0, 0, 1]);
  }
  const headRotation = (yaw, pitch, roll) =>
      multiply(multiply(rotY(-yaw), rotX(pitch)), rotZ(-roll));
  const radius = 2.52;
  const direction = (az, el) => {
    const [ca, sa] = cosSin(az), [ce, se] = cosSin(el);
    return [sa * ce, se, -ca * ce];
  };
  const defaultView = () => ({az: 18, el: 32, zoom: 1});
  function camera(view, width, height) {
    const aspect = width / Math.max(1, height);
    // Fit the full stage on a narrow side panel as well as a wide desktop.
    const tan = Math.tan(23 * rad), distance = 7.35 * Math.max(1, 1 / aspect) / view.zoom;
    const back = direction(180 - view.az, view.el), right = unit(cross([0, 1, 0], back)),
          up = cross(back, right);
    const eye = scale(back, distance);
    const worldToEye = [
      right[0], up[0], back[0], 0, right[1], up[1], back[1], 0, right[2], up[2], back[2], 0,
      -dot(right, eye), -dot(up, eye), -dot(back, eye), 1
    ];
    const near = .1, far = 80;
    const projection = [
      1 / (aspect * tan), 0, 0, 0, 0, 1 / tan, 0, 0, 0, 0, (far + near) / (near - far), -1, 0, 0,
      2 * far * near / (near - far), 0
    ];
    return {
      matrix: multiply(projection, worldToEye),
      eye,
      back,
      right,
      up,
      tan,
      aspect,
      width,
      height
    };
  }
  function project(cam, point) {
    const [x, y, z, w] = transform(cam.matrix, point);
    return {
      x: (x / w + 1) * cam.width / 2,
      y: (1 - y / w) * cam.height / 2,
      visible: w > 0 && Math.abs(z) <= w
    };
  }
  function ray(cam, x, y) {
    const horizontal = (2 * x / cam.width - 1) * cam.tan * cam.aspect,
          vertical = (1 - 2 * y / cam.height) * cam.tan;
    return {
      origin: cam.eye,
      direction:
          unit(add(scale(cam.back, -1), add(scale(cam.right, horizontal), scale(cam.up, vertical))))
    };
  }
  function cabinet(speaker) {
    const isSub = speaker.bit === 3;
    const origin = isSub ? [1.39, -.42, -2.65] : scale(direction(speaker.az, speaker.el), radius);
    const front = unit(scale(origin, -1));
    const right = isSub ? unit(cross([0, 1, 0], front)) : direction(speaker.az + 90, 0);
    const up = unit(cross(front, right));
    const half = isSub ? [.34, .31, .28] : [.229, .335, .178];
    return {
      bit: speaker.bit,
      origin,
      front,
      right,
      up,
      half,
      matrix: axes(right, up, front, origin)
    };
  }
  // Slab test in each cabinet's own coordinates. Choose the nearest visible cabinet.
  function pick(line, boxes) {
    let best = null, nearest = Infinity;
    for (const box of boxes) {
      const relative = sub(line.origin, box.origin);
      let enter = 0, exit = Infinity;
      for (const [i, axis] of [box.right, box.up, box.front].entries()) {
        const p = dot(relative, axis), d = dot(line.direction, axis), extent = box.half[i] + .045;
        if (Math.abs(d) < 1e-8) {
          if (Math.abs(p) > extent)
            exit = -1;
          continue;
        }
        const a = (-extent - p) / d, b = (extent - p) / d;
        enter = Math.max(enter, Math.min(a, b));
        exit = Math.min(exit, Math.max(a, b));
      }
      if (enter <= exit && enter < nearest) {
        best = box.bit;
        nearest = enter;
      }
    }
    return best;
  }
  function spherePoint(line, prior) {
    const b = dot(line.origin, line.direction),
          disc = b * b - dot(line.origin, line.origin) + radius * radius;
    if (disc < 0)
      return null;
    const t = Math.sqrt(disc);
    const hits =
        [-b - t, -b + t].filter(d => d > 0).map(d => add(line.origin, scale(line.direction, d)));
    hits.sort((a, c) => dot(c, prior) - dot(a, prior));
    return hits[0] || null;
  }
  const angles = point => ({
    az: Math.atan2(point[0], -point[2]) / rad,
    el: Math.asin(clamp(unit(point)[1], -1, 1)) / rad
  });
  return {
    rad,
    clamp,
    add,
    sub,
    scale,
    dot,
    cross,
    unit,
    axes,
    identity,
    displacement,
    size,
    multiply,
    transform,
    rotX,
    rotY,
    headRotation,
    radius,
    direction,
    defaultView,
    camera,
    project,
    ray,
    cabinet,
    pick,
    spherePoint,
    angles
  };
})();
if (typeof module !== "undefined" && module.exports)
  module.exports = RoomGeometry;
