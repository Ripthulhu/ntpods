"use strict";
(() => {
  const G = RoomGeometry;
  const viewport = document.getElementById("viewport"), canvas = document.getElementById("stage");
  const overlay = document.getElementById("labels"), screen = document.getElementById("screen"),
        hint = document.getElementById("hint");
  const bridge = window.chrome?.webview;
  const send = m => bridge?.postMessage(m);
  const labels = new Map(),
        shortNames = ["FL", "FR", "C", "LFE", "BL", "BR", "FLC", "FRC", "BC", "SL", "SR"];
  let state = {
    speakers: [],
    yaw: null,
    pitch: null,
    roll: null,
    theme: "dark",
    screen: "",
    hint: ""
  };
  let studio = null, frame = 0, view = G.defaultView(), cam = null, boxes = [], gesture = null,
      pinch = 0;
  const pointers = new Map();
  function invalidate() {
    if (!frame && studio)
      frame = requestAnimationFrame(render);
  }
  function updateCanvas() {
    const width = viewport.clientWidth, height = viewport.clientHeight,
          scale = Math.min(devicePixelRatio || 1, 2);
    if (width < 1 || height < 1)
      return false;
    const w = Math.max(1, Math.round(width * scale)), h = Math.max(1, Math.round(height * scale));
    if (w !== canvas.width || h !== canvas.height) {
      canvas.width = w;
      canvas.height = h;
    }
    cam = G.camera(view, width, height);
    return true;
  }
  function position(element, point, occupied, width) {
    const xy = G.project(cam, point);
    element.hidden = true;
    if (!xy.visible)
      return;
    const margin = width / 2 + 2;
    const offsets = [0, -21, 21, -42, 42];
    const free = offsets.find(dy => {
      const y = xy.y + dy;
      return xy.x >= margin && xy.x <= cam.width - margin && y >= 9 && y <= cam.height - 9
          && occupied.every(
              r => Math.abs(xy.x - r.x) >= margin + r.margin || Math.abs(y - r.y) >= 21);
    });
    if (free === undefined)
      return;
    xy.y += free;
    occupied.push({x: xy.x, y: xy.y, margin});
    element.hidden = false;
    element.style.left = xy.x.toFixed(1) + "px";
    element.style.top = xy.y.toFixed(1) + "px";
  }
  function render() {
    frame = 0;
    if (!studio || !updateCanvas())
      return;
    boxes = state.speakers.map(G.cabinet);
    studio.frame(state, cam, boxes);
    const seen = new Set(), occupied = [];
    for (const [i, s] of state.speakers.entries()) {
      let tag = labels.get(s.bit);
      if (!tag) {
        tag = document.createElement("span");
        tag.className = "speaker-label";
        tag.dataset.bit = String(s.bit);
        labels.set(s.bit, tag);
        overlay.append(tag);
      }
      tag.textContent = shortNames[s.bit] || s.label;
      tag.title = s.name || s.label;
      tag.dataset.selected = String(s.selected);
      tag.dataset.active = String(Boolean(s.active));
      tag.style.setProperty("--strength", G.clamp(((s.level ?? -60) + 60) / 60, 0, 1).toFixed(2));
      // Place labels above cabinets in screen space and use the same bounds for tapping.
      position(
          tag, G.add(boxes[i].origin, G.scale(cam.up, boxes[i].half[1] + .16)), occupied,
          tag.textContent.length * 7 + 10);
      seen.add(s.bit);
    }
    for (const [bit, tag] of labels)
      if (!seen.has(bit)) {
        tag.remove();
        labels.delete(bit);
      }
    screen.textContent = state.screen;
    if (state.screen)
      position(screen, [0, .96, -3.23], occupied, state.screen.length * 7 + 4);
    else
      screen.hidden = true;
  }
  function setRoom(data) {
    const incoming = Array.isArray(data.speakers) ? data.speakers : [], used = new Set();
    const speakers =
        incoming
            .filter(s => {
              if (!s || !Number.isInteger(s.bit) || s.bit < 0 || s.bit > 31 || used.has(s.bit)
                  || !Number.isFinite(s.az) || !Number.isFinite(s.el))
                return false;
              used.add(s.bit);
              return true;
            })
            .map(s => ({
              ...s, az: G.clamp(s.az, -180, 180), el: G.clamp(s.el, -90, 90),
              selected: Boolean(data.selectedMask & (1 << s.bit))
            }));
    if (gesture?.bit != null && !used.has(gesture.bit))
      gesture = null;
    // Single cabinets follow the pointer immediately; groups move together in the host snapshot.
    if (gesture?.moving && gesture.previewSingle && gesture.position) {
      const dragged = speakers.find(s => s.bit === gesture.bit);
      if (dragged)
        Object.assign(dragged, gesture.position);
    }
    state = {
      speakers,
      yaw: data.yaw,
      pitch: data.pitch,
      roll: data.roll,
      theme: ["dark", "light", "contrast"].includes(data.theme) ? data.theme : "dark",
      screen: typeof data.screen === "string" ? data.screen : "",
      hint: typeof data.hint === "string" ? data.hint : ""
    };
    document.documentElement.dataset.theme = state.theme;
    hint.textContent = state.hint;
    if (typeof data.title === "string" && data.title) {
      document.title = data.title;
      canvas.setAttribute("aria-label", data.title);
    }
    if (typeof data.lang === "string" && data.lang)
      document.documentElement.lang = data.lang;
    invalidate();
  }
  function receive(event) {
    const data = event.data;
    if (data?.type === "room")
      setRoom(data);
    else if (data?.type === "resetView") {
      view = G.defaultView();
      invalidate();
    }
  }
  bridge?.addEventListener("message", receive);
  // The same-origin hook also permits standalone, offline previews and test fixtures.
  window.addEventListener("message", event => {
    if (event.source === window || event.source === null)
      receive(event);
  });
  function point(event) {
    const bounds = canvas.getBoundingClientRect();
    return {x: event.clientX - bounds.left, y: event.clientY - bounds.top};
  }
  function hit(x, y) {
    if (!cam)
      return null;
    // Labels sit above the cabinets and can distinguish speakers at the same position.
    const origin = canvas.getBoundingClientRect();
    for (const [bit, tag] of labels) {
      if (tag.hidden)
        continue;
      const r = tag.getBoundingClientRect();
      if (x + origin.left >= r.left && x + origin.left <= r.right
          && y + origin.top >= r.top && y + origin.top <= r.bottom)
        return bit;
    }
    return G.pick(G.ray(cam, x, y), boxes);
  }
  function setZoom(factor) {
    view.zoom = G.clamp(view.zoom * factor, .7, 1.8);
    invalidate();
  }
  function orbit(dx, dy) {
    view.az = (view.az - dx * .35) % 360;
    view.el = G.clamp(view.el + dy * .30, 7, 78);
    invalidate();
  }
  function distance() {
    const [a, b] = pointers.values();
    return Math.hypot(a.x - b.x, a.y - b.y);
  }
  function sendPosition(g, force) {
    if (!g?.position)
      return;
    const now = performance.now();
    if (!force && now - g.lastSend < 65)
      return;
    send({type: "move", bit: g.bit, az: g.position.az, el: g.position.el});
    g.lastSend = now;
  }
  canvas.addEventListener("pointerdown", event => {
    if (event.button !== 0)
      return;
    const p = point(event);
    pointers.set(event.pointerId, p);
    canvas.setPointerCapture(event.pointerId);
    canvas.focus({preventScroll: true});
    if (pointers.size === 2) {
      if (gesture?.moving)
        sendPosition(gesture, true);
      gesture = null;
      pinch = distance();
      return;
    }
    if (pointers.size !== 1)
      return;
    const bit = hit(p.x, p.y), s = state.speakers.find(s => s.bit === bit);
    let grab = null;
    if (cam && s && bit !== 3) {
      const prior = G.scale(G.direction(s.az, s.el), G.radius);
      const onSphere = G.spherePoint(G.ray(cam, p.x, p.y), prior);
      if (onSphere)
        grab = G.angles(onSphere);
    }
    gesture = {
      id: event.pointerId,
      start: p,
      last: p,
      bit,
      toggle: event.ctrlKey,
      previewSingle: !s?.selected || !state.speakers.some(other => other.selected && other.bit !== bit),
      initial: s ? {az: s.az, el: s.el} : null,
      grab,
      lastSend: -Infinity,
      moving: false,
      position: null
    };
  });
  const wrap = deg => ((deg + 180) % 360 + 360) % 360 - 180;
  canvas.addEventListener("pointermove", event => {
    if (!pointers.has(event.pointerId))
      return;
    const p = point(event);
    pointers.set(event.pointerId, p);
    if (pointers.size === 2) {
      const d = distance();
      if (pinch > 1 && d > 1)
        setZoom(d / pinch);
      pinch = d;
      return;
    }
    if (pointers.size !== 1 || !gesture || gesture.id !== event.pointerId)
      return;
    const g = gesture, totalX = p.x - g.start.x, totalY = p.y - g.start.y;
    if (!g.moving) {
      if (Math.hypot(totalX, totalY) < 4)
        return;
      g.moving = true;
      if (g.bit !== null)
        send({type: "select", bit: g.bit, drag: true});
    }
    if (g.bit === null || g.bit === 3)
      orbit(p.x - g.last.x, p.y - g.last.y);
    else if (cam) {
      const previous = g.position || g.initial,
            prior = G.scale(G.direction(previous.az, previous.el), G.radius);
      const onSphere = g.grab && G.spherePoint(G.ray(cam, p.x, p.y), prior);
      const delta = onSphere ? G.angles(onSphere) : null;
      const az =
          delta ? g.initial.az + wrap(delta.az - g.grab.az) : previous.az + (p.x - g.last.x) * .4;
      const el = delta ? g.initial.el + delta.el - g.grab.el : previous.el - (p.y - g.last.y) * .28;
      g.position = {az: Math.round(wrap(az)), el: Math.round(G.clamp(el, -90, 90))};
      const s = state.speakers.find(s => s.bit === g.bit);
      if (s && g.previewSingle)
        Object.assign(s, g.position);
      invalidate();
      sendPosition(g, false);
    }
    g.last = p;
  });
  function end(event, cancelled) {
    if (!pointers.has(event.pointerId))
      return;
    const g = gesture;
    if (g?.id === event.pointerId) {
      if (g.moving)
        sendPosition(g, true);
      else if (!cancelled && g.bit !== null)
        send({type: "select", bit: g.bit, toggle: g.toggle, edit: true});
    }
    pointers.delete(event.pointerId);
    gesture = null;
    pinch = 0;
  }
  canvas.addEventListener("pointerup", e => end(e, false));
  canvas.addEventListener("pointercancel", e => end(e, true));
  canvas.addEventListener("lostpointercapture", e => end(e, true));
  canvas.addEventListener("wheel", e => {
    e.preventDefault();
    setZoom(Math.exp(-G.clamp(e.deltaY * (e.deltaMode === 1 ? 16 : 1), -300, 300) * .0015));
  }, {passive: false});
  canvas.addEventListener("dblclick", e => {
    const p = point(e);
    if (hit(p.x, p.y) === null) {
      view = G.defaultView();
      invalidate();
    }
  });
  canvas.addEventListener("keydown", e => {
    switch (e.key) {
      case "ArrowLeft":
        orbit(15, 0);
        break;
      case "ArrowRight":
        orbit(-15, 0);
        break;
      case "ArrowUp":
        orbit(0, 15);
        break;
      case "ArrowDown":
        orbit(0, -15);
        break;
      case "+":
      case "=":
        setZoom(1.10);
        break;
      case "-":
        setZoom(1 / 1.10);
        break;
      case "Home":
        view = G.defaultView();
        invalidate();
        break;
      default:
        return;
    }
    e.preventDefault();
  });
  new ResizeObserver(invalidate).observe(viewport);
  function init() {
    try {
      studio = createStudio(canvas);
      invalidate();
      send({type: "ready"});
    } catch (e) {
      studio = null;
      console.error(e);
      send({type: "unavailable"});
    }
  }
  canvas.addEventListener("webglcontextlost", e => {
    e.preventDefault();
    studio = null;
    send({type: "unavailable"});
  });
  canvas.addEventListener("webglcontextrestored", init);
  init();
})();
