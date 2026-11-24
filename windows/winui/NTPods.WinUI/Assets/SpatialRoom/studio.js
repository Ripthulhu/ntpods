"use strict";
// Static indexed meshes live in GPU buffers. All channel activity and pose updates reuse them.
function createStudio(canvas) {
  const gl =
      canvas.getContext("webgl2", {alpha: false, antialias: true, powerPreference: "low-power"});
  if (!gl)
    throw Error("WebGL2 unavailable");
  const G = RoomGeometry, mul = G.multiply, place = G.displacement, size = G.size;
  const vertex = `#version 300 es
    layout(location = 0) in vec3 position;
    layout(location = 1) in vec3 normal;
    uniform mat4 vp, model;
    out vec3 n, p, local;

    void main() {
      vec4 world = model * vec4(position, 1.0);
      local = position;
      p = world.xyz;
      n = transpose(inverse(mat3(model))) * normal;
      gl_Position = vp * world;
    }`;
  const fragment = `#version 300 es
    precision highp float;
    in vec3 n, p, local;
    uniform vec3 tint, eye, bg;
    uniform float finish, glow;
    uniform int kind;
    out vec4 color;

    vec3 studioFloor() {
      float radius = length(local.xz);
      float fade = 1.0 - smoothstep(0.70, 1.0, radius);
      vec2 cells = p.xz * 2.0;
      vec2 lines = abs(fract(cells - 0.5) - 0.5) / max(fwidth(cells), vec2(0.0001));
      float grid = 1.0 - smoothstep(0.0, 1.15, min(lines.x, lines.y));
      float roomRadius = length(p.xz);
      float ring = 1.0 - smoothstep(0.005, 0.005 + fwidth(roomRadius) * 1.25,
                                    abs(roomRadius - 2.52));
      vec3 base = mix(bg, tint, fade * (0.40 + 0.25 * (1.0 - radius)));
      vec3 ink = mix(tint, vec3(0.43, 0.62, 0.78), 0.65);
      return mix(base, ink, fade * (grid * 0.21 + ring * 0.43));
    }

    vec3 shade() {
      vec3 N = normalize(n);
      vec3 V = normalize(eye - p);
      vec3 L = normalize(vec3(-0.52, 0.93, 0.63));
      float diffuse = max(dot(N, L), 0.0);
      float hemisphere = 0.5 + 0.5 * N.y;
      float rim = pow(1.0 - max(dot(N, V), 0.0), 2.8);
      float specular = pow(max(dot(N, normalize(V + L)), 0.0), 30.0);
      vec3 fill = vec3(0.10, 0.17, 0.24) * max(dot(N, normalize(vec3(0.83, 0.5, -0.46))), 0.0);
      vec3 lit = tint * (0.54 + 0.31 * hemisphere + 0.57 * diffuse + fill)
               + finish * (vec3(0.40, 0.44, 0.48) * specular + vec3(0.17, 0.22, 0.29) * rim);
      return mix(lit, tint, glow);
    }

    void main() {
      if (kind == 1) {
        color = vec4(studioFloor(), 1.0);
      } else if (kind == 2) {
        float distanceSquared = dot(local.xz, local.xz);
        color = vec4(tint, (1.0 - smoothstep(0.0, 1.0, distanceSquared)) * 0.45);
      } else if (kind == 3) {
        float radius = length(local.xz);
        float antialias = max(fwidth(radius), 0.012);
        float ring = 1.0 - smoothstep(0.028, 0.028 + antialias, abs(radius - 0.82));
        float center = 1.0 - smoothstep(0.10, 0.84, radius);
        color = vec4(tint, glow * (ring + 0.25 * center));
      } else {
        color = vec4(shade(), 1.0);
      }
    }`;
  function shader(type, source) {
    const s = gl.createShader(type);
    gl.shaderSource(s, source);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS))
      throw Error(gl.getShaderInfoLog(s));
    return s;
  }
  const program = gl.createProgram();
  const v = shader(gl.VERTEX_SHADER, vertex), f = shader(gl.FRAGMENT_SHADER, fragment);
  gl.attachShader(program, v);
  gl.attachShader(program, f);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS))
    throw Error(gl.getProgramInfoLog(program));
  gl.deleteShader(v);
  gl.deleteShader(f);
  gl.useProgram(program);
  const u = Object.fromEntries(["vp", "model", "tint", "eye", "bg", "finish", "glow", "kind"].map(
      k => [k, gl.getUniformLocation(program, k)]));
  function mesh(points, indices) {
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const b = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, b);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(points), gl.STATIC_DRAW);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 24, 0);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 24, 12);
    gl.enableVertexAttribArray(1);
    const e = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, e);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint16Array(indices), gl.STATIC_DRAW);
    return {vao, count: indices.length};
  }
  function surface(nh, nv, sample) {
    const p = [], ix = [];
    for (let j = 0; j <= nv; j++)
      for (let i = 0; i <= nh; i++)
        p.push(...sample(i / nh, j / nv));
    for (let j = 0; j < nv; j++)
      for (let i = 0; i < nh; i++) {
        const a = j * (nh + 1) + i;
        ix.push(a, a + 1, a + nh + 1, a + 1, a + nh + 2, a + nh + 1);
      }
    return mesh(p, ix);
  }
  const sphere = surface(28, 18, (u, v) => {
    const az = 2 * Math.PI * u, el = Math.PI * (v - .5),
          p = [Math.sin(az) * Math.cos(el), Math.sin(el), Math.cos(az) * Math.cos(el)];
    return [...p, ...p];
  });
  function roundBox() {
    const verts = [], indices = [], stops = [-1, -.97, -.91, -.82, .82, .91, .97, 1];
    for (let axis = 0; axis < 3; axis++)
      for (const sign of [-1, 1]) {
        const start = verts.length / 6;
        for (const j of stops)
          for (const i of stops) {
            const p = [0, 0, 0];
            p[axis] = sign;
            p[(axis + 1) % 3] = i;
            p[(axis + 2) % 3] = j;
            const inner = p.map(k => G.clamp(k, -.82, .82)), n = G.unit(G.sub(p, inner));
            verts.push(...G.add(inner, G.scale(n, .18)), ...n);
          }
        for (let j = 0; j < 7; j++)
          for (let i = 0; i < 7; i++) {
            const a = start + j * 8 + i;
            indices.push(a, a + 1, a + 8, a + 1, a + 9, a + 8);
          }
      }
    return mesh(verts, indices);
  }
  const box = roundBox();
  // Loft horizontal profiles into one cast surface. The neck, jaw and shoulders
  // have different depths front-to-back, visible even from behind the listener.
  function loft(rows) {
    return surface(40, rows.length - 1, (u, v) => {
      const i = Math.round(v * (rows.length - 1)), [y, rx, rz, z] = rows[i];
      const lo = rows[Math.max(0, i - 1)], hi = rows[Math.min(rows.length - 1, i + 1)];
      const angle = 2 * Math.PI * u, c = Math.cos(angle), s = Math.sin(angle);
      const tangent = [-rx * s, 0, rz * c];
      const rise = [(hi[1] - lo[1]) * c, hi[0] - lo[0], (hi[2] - lo[2]) * s + hi[3] - lo[3]];
      return [rx * c, y, rz * s + z, ...G.unit(G.cross(rise, tangent))];
    });
  }
  const shoulders = loft([
    [-.78, .40, .22, .07], [-.73, .52, .245, .08], [-.64, .55, .25, .08], [-.59, .48, .24, .08],
    [-.55, .37, .21, .07], [-.49, .24, .18, .03], [-.43, .145, .15, .015], [-.27, .148, .16, .01]
  ]);
  const skull = loft([
    [-.34, .045, .08, -.105], [-.305, .13, .16, -.09], [-.24, .205, .213, -.052],
    [-.13, .264, .264, -.025], [.035, .30, .305, .006], [.16, .308, .31, .028],
    [.27, .287, .28, .046], [.36, .225, .226, .055], [.415, .125, .145, .053],
    [.432, .005, .005, .048]
  ]);
  const plane = surface(1, 1, (u, v) => [2 * u - 1, 0, 2 * v - 1, 0, 1, 0]);
  // Revolve a loudspeaker cross-section around local +Z, including the inward-sloping cone.
  function driver(profile) {
    const p = [], ix = [], sectors = 32;
    for (let j = 0; j < profile.length; j++) {
      const [r, z] = profile[j], before = profile[Math.max(0, j - 1)],
                after = profile[Math.min(profile.length - 1, j + 1)];
      const dz = after[1] - before[1], dr = after[0] - before[0];
      for (let i = 0; i <= sectors; i++) {
        const a = 2 * Math.PI * i / sectors, c = Math.cos(a), s = Math.sin(a);
        p.push(r * c, r * s, z, ...G.unit([-dz * c, -dz * s, dr]));
      }
    }
    for (let j = 0; j < profile.length - 1; j++)
      for (let i = 0; i < sectors; i++) {
        const a = j * (sectors + 1) + i;
        ix.push(a, a + 1, a + sectors + 1, a + 1, a + sectors + 2, a + sectors + 1);
      }
    return mesh(p, ix);
  }
  const cone = driver(
      [[0, .13], [.18, .23], [.32, .03], [.7, .18], [.78, .33], [.85, .38], [.93, .26], [1, .12]]);
  const trim = driver([[.87, .12], [.94, .21], [1, .12], [1, 0]]);
  const I = G.identity();
  let colors;
  function draw(shape, model, color, finish = .45, glow = 0, kind = 0) {
    gl.uniformMatrix4fv(u.model, false, model);
    gl.uniform3fv(u.tint, color);
    gl.uniform1f(u.finish, finish);
    gl.uniform1f(u.glow, glow);
    gl.uniform1i(u.kind, kind);
    gl.bindVertexArray(shape.vao);
    gl.drawElements(gl.TRIANGLES, shape.count, gl.UNSIGNED_SHORT, 0);
  }
  function at(root, x, y, z, sx, sy, sz) {
    return mul(root, mul(place(x, y, z), size(sx, sy, sz)));
  }
  function part(root, shape, x, y, z, sx, sy, sz, color, finish, glow) {
    draw(shape, at(root, x, y, z, sx, sy, sz), color, finish, glow);
  }
  function shadow(x, y, z, rx, rz) {
    draw(plane, at(I, x, y, z, rx, 1, rz), colors.shadow, 0, 0, 2);
  }
  const palettes = {
    dark: {
      bg: [.063, .102, .145],
      floor: [.15, .22, .29],
      cab: [.18, .23, .28],
      baffle: [.09, .13, .17],
      rim: [.36, .43, .50],
      cone: [.16, .19, .22],
      metal: [.33, .41, .48],
      head: [.71, .74, .73],
      ear: [.53, .57, .58],
      blue: [.29, .69, 1],
      dim: [.17, .24, .29],
      screen: [.12, .20, .27],
      shadow: [.015, .026, .036]
    },
    light: {
      bg: [.89, .92, .94],
      floor: [.67, .74, .8],
      cab: [.26, .32, .38],
      baffle: [.13, .18, .22],
      rim: [.49, .55, .60],
      cone: [.19, .22, .25],
      metal: [.44, .52, .59],
      head: [.76, .71, .66],
      ear: [.58, .55, .52],
      blue: [.02, .46, .78],
      dim: [.25, .31, .35],
      screen: [.36, .44, .51],
      shadow: [.25, .31, .37]
    },
    contrast: {
      bg: [.012, .012, .012],
      floor: [.24, .24, .24],
      cab: [.41, .41, .41],
      baffle: [.09, .09, .09],
      rim: [.86, .86, .86],
      cone: [.38, .38, .38],
      metal: [.68, .68, .68],
      head: [.86, .86, .86],
      ear: [.56, .56, .56],
      blue: [.25, .85, 1],
      dim: [.43, .43, .43],
      screen: [.55, .55, .55],
      shadow: [0, 0, 0]
    }
  };
  function makeHead(state) {
    const live = Number.isFinite(state.yaw);
    // The neck and shoulders remain in the room. Only the head follows the reported pose.
    draw(shoulders, I, colors.metal, .5);
    const h = live ? G.headRotation(state.yaw, state.pitch ?? 0, state.roll ?? 0) : I;
    draw(skull, h, colors.head, .42);
    part(h, sphere, 0, .005, -.29, .066, .128, .105, colors.head, .31);
    part(h, sphere, 0, -.052, -.354, .071, .052, .073, colors.head, .31);
    for (const side of [-1, 1]) {
      part(h, sphere, side * .297, -.012, .016, .063, .114, .074, colors.ear);
      part(h, sphere, side * .342, -.016, .018, .029, .067, .042, colors.cab);
      part(h, sphere, side * .355, -.019, .024, .032, .041, .034, colors.head, .9);
      part(h, box, side * .36, -.079, .025, .019, .059, .019, colors.head, .9);
    }
    // A thin top fin points toward the nose and remains visible from behind the head.
    const mark = live ? colors.blue : colors.dim;
    part(h, box, 0, .435, -.054, .014, .008, .113, mark, .4, .65);
    for (const side of [-1, 1]) {
      const wing = mul(h, mul(place(side * .024, .436, -.142), G.rotY(-side * 37)));
      part(wing, box, 0, 0, 0, .013, .008, .045, mark, .4, .65);
    }
  }
  function makeCabinet(s, boxData, state, floor) {
    const b = boxData.matrix, [w, h, d] = boxData.half, selected = state.selected === s.bit;
    const supportHeight = boxData.origin[1] - h - floor;
    if (supportHeight > .10) {
      part(
          I, box, boxData.origin[0], floor + supportHeight / 2, boxData.origin[2], .023,
          supportHeight / 2, .023, selected ? colors.rim : colors.metal);
      part(
          I, box, boxData.origin[0], floor + .018, boxData.origin[2], .14, .015, .12, colors.metal);
    }
    part(b, box, 0, 0, 0, w, h, d, colors.cab, .8);
    part(b, box, 0, 0, d * .93, w * .93, h * .95, d * .13, colors.baffle, .45);
    part(b, box, 0, 0, -d * .989, w * .81, h * .83, d * .034, colors.baffle, .5);
    for (let j = -2; j <= 2; j++)
      part(b, box, j * .04, .05, -d * 1.036, .009, .09, .003, colors.cone, .55);
    part(b, box, 0, -h * .48, -d * 1.036, w * .22, .017, .004, colors.rim, .8);
    function transducer(y, r) {
      part(b, trim, 0, y, d * 1.055, r, r, r * .23, colors.rim, .95);
      part(b, cone, 0, y, d * 1.065, r * .89, r * .89, r * .24, colors.cone, .7);
    }
    if (s.bit === 3)
      transducer(0, w * .76);
    else {
      transducer(-.075, w * .72);
      transducer(.155, w * .33);
    }
    // The rear indicator remains visible on the speakers facing away from the camera.
    const level = Number.isFinite(s.level) ? G.clamp((s.level + 60) / 60, 0, 1) : 0;
    const lamps = s.active ? Math.max(1, Math.ceil(level * 4)) : 0;
    for (const face of [-1, 1])
      for (let j = 0; j < 4; j++) {
        part(
            b, box, (j - 1.5) * .055, -h * .74, face * d * (face > 0 ? 1.075 : 1.047), .021, .015,
            .005, j < lamps ? colors.blue : colors.dim, .2, j < lamps ? .9 : 0);
      }
    if (selected) {
      part(b, box, 0, h * 1.012, 0, w * .72, .006, d * .67, colors.blue, .1, .7);
    }
  }
  function frame(state, cam, boxes) {
    colors = palettes[state.theme] || palettes.dark;
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.enable(gl.DEPTH_TEST);
    gl.clearColor(...colors.bg, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.uniformMatrix4fv(u.vp, false, cam.matrix);
    gl.uniform3fv(u.eye, cam.eye);
    gl.uniform3fv(u.bg, colors.bg);
    const floor = Math.min(-.87, ...boxes.map(b => b.origin[1] - b.half[1] - .08));
    draw(plane, at(I, 0, floor, 0, 4.35, 1, 4.35), colors.floor, 0, 0, 1);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.depthMask(false);
    shadow(0, floor + .008, .05, .71, .48);
    boxes.forEach(b => shadow(b.origin[0], floor + .008, b.origin[2], .46, .35));
    boxes.forEach((b, i) => {
      const s = state.speakers[i];
      if (!s.active)
        return;
      const strength = G.clamp(((s.level ?? -60) + 60) / 60, 0, 1);
      const spread = .36 + .14 * strength;
      draw(
          plane, at(I, b.origin[0], floor + .012, b.origin[2], spread, 1, spread), colors.blue, 0,
          .25 + .67 * strength, 3);
    });
    gl.depthMask(true);
    gl.disable(gl.BLEND);
    // A thin display frame marks the room front, visually distinct from the loudspeakers.
    const screenZ = -3.23, screenY = .23, rim = colors.metal;
    part(I, box, 0, screenY, screenZ, 1.30, .48, .037, colors.screen, .6);
    part(I, box, 0, screenY, screenZ + .035, 1.26, .44, .013, colors.bg, 0, .45);
    part(I, box, 0, screenY - .49, screenZ, .36, .009, .041, rim, .6);
    part(I, box, 0, screenY - .575, screenZ, .041, .093, .029, rim, .6);
    part(I, box, 0, screenY - .67, screenZ, .23, .012, .09, rim, .6);
    makeHead(state);
    boxes.forEach((b, i) => makeCabinet(state.speakers[i], b, state, floor));
  }
  return {frame};
}
