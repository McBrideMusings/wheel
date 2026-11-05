// Wheel's lightbox for an artifact page: a click on an <img> opens it full window; a second click
// zooms, a drag pans, Esc closes, the arrow keys step between the page's images.
// A page asks for it: <script src=".wheel/lightbox.js"></script>. Once loaded it opens every <img>
// except one marked data-no-lightbox or one inside a link. The caption is the nearest data-cap, else
// the image's alt. window.wheelLightbox lets a page that redraws its images keep it open.
(() => {
  if (window.wheelLightbox) return;
  const MARGIN = 32;
  const css = `
  #wheel-lightbox { position: fixed; inset: 0; z-index: 2147483000; overflow: hidden; touch-action: none; }
  #wheel-lightbox[hidden] { display: none; }
  #wheel-lightbox .dim { position: absolute; inset: 0; background: rgba(0, 0, 0, 0.82); opacity: 0; transition: opacity 200ms ease; }
  #wheel-lightbox.on .dim { opacity: 1; }
  #wheel-lightbox img {
    position: absolute; max-width: none; user-select: none; -webkit-user-select: none; -webkit-user-drag: none;
    cursor: zoom-in; transition: left 200ms ease, top 200ms ease, width 200ms ease, height 200ms ease;
    box-shadow: 0 8px 40px rgba(0, 0, 0, 0.5); background: var(--group, #222);
  }
  #wheel-lightbox.zoomed img { cursor: grab; }
  #wheel-lightbox.dragging img { cursor: grabbing; transition: none; }
  #wheel-lightbox .cap {
    position: absolute; left: 0; right: 0; bottom: 8px; text-align: center; color: #f5f5f7; font: 12px -apple-system, system-ui, sans-serif;
    opacity: 0; transition: opacity 200ms ease; pointer-events: none; text-shadow: 0 1px 3px #000;
  }
  #wheel-lightbox.on .cap { opacity: 1; }
  @media (prefers-reduced-motion: reduce) { #wheel-lightbox .dim, #wheel-lightbox img, #wheel-lightbox .cap { transition: none; } }`;

  let spot = null, spotImg = null, spotCap = null;
  let spotOpen = false, cur = 0, zoomed = false, closeTimer = 0;
  let box = { w: 0, h: 0, cx: 0, cy: 0 }; // image size and centre, in window px
  let nat = { w: 1, h: 1 };
  let pan = { x: 0, y: 0 }; // offset of the centre from the window centre while zoomed
  let down = null;

  const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const vw = () => document.documentElement.clientWidth || window.innerWidth;
  const vh = () => window.innerHeight;
  const eligible = (img) =>
    !!(img.currentSrc || img.src) && !img.hasAttribute("data-no-lightbox") && !img.closest("#wheel-lightbox, a[href]");
  const items = () => Array.from(document.querySelectorAll("img")).filter(eligible);
  const keyOf = (img) => img.currentSrc || img.src;
  const capOf = (img) => {
    const holder = img.closest("[data-cap]");
    return holder ? holder.dataset.cap : img.alt || "";
  };

  function build() {
    if (spot) return;
    const style = document.createElement("style");
    style.textContent = css;
    document.head.appendChild(style);
    spot = document.createElement("div");
    spot.id = "wheel-lightbox";
    spot.hidden = true;
    spot.setAttribute("role", "dialog");
    spot.setAttribute("aria-modal", "true");
    spot.setAttribute("aria-label", "Image");
    spot.innerHTML = '<div class="dim"></div><img alt=""><div class="cap"></div>';
    document.body.appendChild(spot);
    spotImg = spot.querySelector("img");
    spotCap = spot.querySelector(".cap");
    wire();
  }

  function fitted() {
    const aw = Math.max(vw() - 2 * MARGIN, 50), ah = Math.max(vh() - 2 * MARGIN, 50);
    const s = Math.min(aw / nat.w, ah / nat.h);
    return { w: nat.w * s, h: nat.h * s };
  }
  function zoomSize() {
    const f = fitted();
    const k = Math.max(1, nat.w / f.w); // natural size as a multiple of fitted
    const m = k < 2 ? 2 : k;
    return { w: f.w * m, h: f.h * m };
  }
  function clampPan() {
    const lim = (size, view) => Math.max(0, (size - view) / 2);
    pan.x = Math.min(Math.max(pan.x, -lim(box.w, vw())), lim(box.w, vw()));
    pan.y = Math.min(Math.max(pan.y, -lim(box.h, vh())), lim(box.h, vh()));
  }
  function place() {
    const cx = vw() / 2 + (zoomed ? pan.x : 0), cy = vh() / 2 + (zoomed ? pan.y : 0);
    box.cx = cx; box.cy = cy;
    spotImg.style.width = box.w + "px";
    spotImg.style.height = box.h + "px";
    spotImg.style.left = cx - box.w / 2 + "px";
    spotImg.style.top = cy - box.h / 2 + "px";
  }
  function layout() {
    const s = zoomed ? zoomSize() : fitted();
    box.w = s.w; box.h = s.h;
    if (zoomed) clampPan();
    place();
  }
  const rectOf = (img) => {
    const r = img.getBoundingClientRect();
    return { w: r.width, h: r.height, cx: r.left + r.width / 2, cy: r.top + r.height / 2 };
  };
  function setImage(img, thenFit) {
    spotImg.alt = img.alt;
    spotImg.src = keyOf(img);
    spotCap.textContent = capOf(img);
    nat = { w: img.naturalWidth || 1, h: img.naturalHeight || 1 };
    if (!img.naturalWidth) {
      // Not decoded yet: take the size when it is.
      img.addEventListener("load", () => {
        if (spotOpen && items()[cur] === img) { nat = { w: img.naturalWidth, h: img.naturalHeight }; layout(); }
      }, { once: true });
    }
    if (thenFit) layout();
  }

  function open(img) {
    const i = items().indexOf(img);
    if (i < 0) return false;
    build();
    clearTimeout(closeTimer);
    cur = i;
    zoomed = false; pan = { x: 0, y: 0 };
    spot.hidden = false;
    spotOpen = true;
    spot.classList.remove("zoomed", "dragging", "on");
    // Start where the image is on the page, then grow to the fitted size.
    const tb = rectOf(img);
    setImage(img, false);
    spotImg.style.transition = "none";
    spotImg.style.width = tb.w + "px"; spotImg.style.height = tb.h + "px";
    spotImg.style.left = tb.cx - tb.w / 2 + "px"; spotImg.style.top = tb.cy - tb.h / 2 + "px";
    void spotImg.offsetWidth;
    spotImg.style.transition = "";
    spot.classList.add("on");
    layout();
    document.body.style.overflow = "hidden";
    spot.setAttribute("tabindex", "-1");
    spot.focus({ preventScroll: true });
    return true;
  }

  function step(d) {
    const all = items();
    if (all.length < 2) return;
    cur = (cur + d + all.length) % all.length;
    zoomed = false; pan = { x: 0, y: 0 };
    spot.classList.remove("zoomed");
    const prev = spotImg.style.transition;
    spotImg.style.transition = "none";
    setImage(all[cur], true);
    void spotImg.offsetWidth;
    spotImg.style.transition = prev;
    all[cur].scrollIntoView({ block: "nearest" });
  }

  function close(immediate) {
    if (!spotOpen) return;
    spotOpen = false;
    const img = items()[cur];
    const finish = () => {
      spot.hidden = true;
      spot.classList.remove("on", "zoomed", "dragging");
      spotImg.removeAttribute("src");
      document.body.style.overflow = "";
    };
    const back = img && (img.closest("button, [tabindex]") || img);
    if (back && back.focus) back.focus({ preventScroll: true });
    if (immediate || reduced() || !img) { finish(); return; }
    spot.classList.remove("on", "zoomed");
    const tb = rectOf(img);
    spotImg.style.width = tb.w + "px"; spotImg.style.height = tb.h + "px";
    spotImg.style.left = tb.cx - tb.w / 2 + "px"; spotImg.style.top = tb.cy - tb.h / 2 + "px";
    closeTimer = setTimeout(finish, 210);
  }

  function toggleZoom(px, py) {
    if (!zoomed) {
      const before = { w: box.w, cx: box.cx, cy: box.cy };
      zoomed = true;
      const z = zoomSize();
      const r = z.w / before.w;
      // Keep the point under the pointer where it is.
      pan = { x: (before.cx - px) * r + px - vw() / 2, y: (before.cy - py) * r + py - vh() / 2 };
      spot.classList.add("zoomed");
    } else {
      zoomed = false; pan = { x: 0, y: 0 };
      spot.classList.remove("zoomed");
    }
    layout();
  }

  function wire() {
    // Pointer: a press that moves more than a few pixels is a drag, never a click.
    spotImg.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      down = { id: e.pointerId, x: e.clientX, y: e.clientY, px: pan.x, py: pan.y, moved: false };
      try { spotImg.setPointerCapture(e.pointerId); } catch (_) {}
      e.preventDefault();
    });
    spotImg.addEventListener("pointermove", (e) => {
      if (!down || e.pointerId !== down.id) return;
      const dx = e.clientX - down.x, dy = e.clientY - down.y;
      if (!down.moved && Math.hypot(dx, dy) < 4) return;
      down.moved = true;
      if (!zoomed) return;
      spot.classList.add("dragging");
      pan.x = down.px + dx; pan.y = down.py + dy;
      clampPan();
      place();
    });
    const release = (e) => {
      if (!down || e.pointerId !== down.id) return;
      const d = down;
      down = null;
      spot.classList.remove("dragging");
      try { spotImg.releasePointerCapture(e.pointerId); } catch (_) {}
      if (e.type === "pointerup" && !d.moved) toggleZoom(e.clientX, e.clientY);
    };
    spotImg.addEventListener("pointerup", release);
    spotImg.addEventListener("pointercancel", release);
    spotImg.addEventListener("click", (e) => e.stopPropagation());
    spotImg.addEventListener("dragstart", (e) => e.preventDefault());
    // A click on the dark background closes; the image handles its own.
    spot.addEventListener("click", (e) => { if (e.target !== spotImg) close(false); });
    spot.addEventListener("wheel", (e) => {
      e.preventDefault();
      if (!zoomed) return;
      pan.x -= e.deltaX; pan.y -= e.deltaY;
      clampPan();
      spotImg.style.transition = "none";
      place();
      void spotImg.offsetWidth;
      spotImg.style.transition = "";
    }, { passive: false });
    document.addEventListener("keydown", (e) => {
      if (!spotOpen) return;
      if (e.key === "Escape") { e.preventDefault(); close(false); }
      else if (e.key === "ArrowLeft") { e.preventDefault(); step(-1); }
      else if (e.key === "ArrowRight") { e.preventDefault(); step(1); }
      else if (e.key === "Tab") { e.preventDefault(); }
    });
    window.addEventListener("resize", () => {
      if (!spotOpen) return;
      spotImg.style.transition = "none"; layout(); void spotImg.offsetWidth; spotImg.style.transition = "";
    });
  }

  // A click on an image opens it. A keyboard press on a button that holds one image arrives as a
  // click on the button, so that opens it too.
  document.addEventListener("click", (e) => {
    const t = e.target;
    if (!(t instanceof Element) || t.closest("#wheel-lightbox")) return;
    let img = t.closest("img");
    if (!img) {
      const btn = t.closest("button");
      const own = btn && btn.querySelectorAll("img");
      img = own && own.length === 1 ? own[0] : null;
    }
    if (img && eligible(img) && open(img)) e.preventDefault();
  });

  window.wheelLightbox = {
    open,
    close,
    isOpen: () => spotOpen,
    // The key of the open image, to hand to show() after the page redraws its images.
    key: () => (spotOpen && items()[cur] ? keyOf(items()[cur]) : null),
    // Keep the lightbox open on the image with `key` after a redraw; false when it is gone.
    show: (key) => {
      const i = items().findIndex((img) => keyOf(img) === key);
      if (!spotOpen || i < 0) return false;
      cur = i;
      setImage(items()[cur], true);
      return true;
    },
  };
})();
