// Shared list and sheet helper for the artifact pages. A page loads it with <script src="vlist.js"></script>.
//   VList.mount(host, { id, count, key(i), row(i) -> html of one .row, rowH | () => px, maxRows = 12, onOpen(i, event), onKey(event, i) -> true if handled })
//     Draws a fixed-row-height list into `host` (an empty div) that renders only the rows in view plus an overscan, so
//     a list of 10,000 holds about 30 rows in the DOM. The scroll position and the active row are kept under `id`
//     across redraws (a page may replace the host with new HTML on every snapshot). Arrow keys, Home, End and Page
//     keys move the active row, Return opens it. Returns { update(patch), redraw(), reveal(key), host }.
//   VList.sheet({ onClose }) -> { el, body, set({ head, nav, body, acts, mount }), close(), isOpen() }
//     The one dialog every page uses: a centred sheet of height min(88vh, 880px) and width min(760px, 100% - 28px), its
//     header pinned, its body the only scroll area, its actions pinned under it. Esc and a click outside close it and
//     focus returns to the element that opened it.
//   VList.preserve(fn): runs fn (a redraw that replaces HTML) and puts back the focused field, its caret and the page scroll.
//   VList.fmt(n) groups thousands; VList.esc(s) escapes HTML; VList.blank(title, text) is the centred empty state.
(function (g) {
  const SHEET_CSS = `
.vl { position: relative; overflow-x: hidden; overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; }
.vl:focus { outline: none; }
.vl:focus-visible { outline: none; box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--accent) 55%, transparent); }
.vl-in { position: relative; width: 100%; }
.vl-win { position: absolute; left: 0; right: 0; top: 0; }
.vl .vl-win > .row { height: var(--vh) !important; min-height: 0 !important; overflow: hidden; border-top: 1px solid var(--separator); border-radius: 0; }
.vl .vl-win > .row.vl-first { border-top: 0; }
.vl .vl-win > .row:hover { background: var(--fill); }
.vl .vl-win > .row.kb { background: var(--fill); }
.vl:focus .vl-win > .row.kb { background: var(--accent-fill); }
.group > .vl { border-top: 1px solid var(--separator); }
.group > .vl:first-child { border-top: 0; }
.linkbtn { all: unset; cursor: pointer; color: var(--accent); margin-left: 10px; display: inline-block; min-width: 5.2em; text-align: right; }   /* Show more and Show fewer are one width, so the button does not move when pressed */
.linkbtn:hover { text-decoration: underline; }
.linkbtn:focus-visible { outline: 3px solid color-mix(in srgb, var(--accent) 55%, transparent); outline-offset: 2px; border-radius: 3px; }
.heading { white-space: nowrap; min-height: 19px; }
.heading .hint .cnt { font-variant-numeric: tabular-nums; }
.heading .hint { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
@media (max-width: 520px) { .heading .hint .cnt { display: none; } .heading .hint .linkbtn { margin-left: 0; } }
.vl-more { display: flex; align-items: center; justify-content: space-between; padding: 0 12px; height: 28px; font-size: var(--callout); color: var(--secondary); }

.sheet-back { position: fixed; inset: 0; z-index: 50; background: rgba(0, 0, 0, .45); display: flex; align-items: center; justify-content: center; }
.sheet-back[hidden] { display: none; }
.sheet-body:focus { outline: none; }
.sheet { width: min(760px, calc(100% - 28px)); height: min(88vh, 880px); display: flex; flex-direction: column; background: var(--group); border: 1px solid var(--separator-strong);
  border-radius: 12px; box-shadow: 0 16px 48px rgba(0, 0, 0, .45); outline: none; overflow: hidden; }
.sheet-head { flex: none; display: flex; align-items: center; gap: 10px; min-height: 44px; padding: 8px 12px 8px 16px; border-bottom: 1px solid var(--separator); }
.sheet-l { flex: 1; min-width: 0; display: flex; align-items: center; gap: 8px; font-size: var(--callout); color: var(--secondary); }
.sheet-r { flex: none; display: flex; align-items: center; gap: 6px; }
.sheet-nav { display: inline-flex; gap: 6px; }
.sheet-head button { height: 26px; padding-top: 0; padding-bottom: 0; font-size: var(--callout); }
.sheet-body { flex: 1; min-height: 0; overflow-x: hidden; overflow-y: auto; padding: 14px 18px 20px; overscroll-behavior: contain; }
.sheet-body > h2, .sheet-body > h3 { margin: 0 0 8px; font-size: 17px; line-height: 1.3; font-weight: 600; overflow-wrap: anywhere; }
.sheet-body dl { display: grid; grid-template-columns: 96px minmax(0, 1fr); gap: 4px 12px; margin: 8px 0 12px; font-size: var(--callout); }
.sheet-body dt { color: var(--secondary); }
.sheet-body dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
.sheet-acts { flex: none; display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; padding: 10px 12px; border-top: 1px solid var(--separator); }
.sheet-acts:empty { display: none; }
.sheet-acts button { font-size: var(--callout); }
.sect { margin: 16px 0 6px; font-size: var(--callout); font-weight: 600; color: var(--secondary); }
.sect .n { font-weight: 400; color: var(--tertiary); font-variant-numeric: tabular-nums; }
.blank { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px; min-height: 50vh; padding: 0 16px; text-align: center; color: var(--secondary); }
.blank.inline { min-height: 160px; }
.blank b { color: var(--label); font-size: var(--body); font-weight: 600; }
.empty-row { padding: 12px; color: var(--secondary); font-size: var(--callout); }
`;
  const style = document.createElement('style');
  style.textContent = SHEET_CSS;
  document.head.appendChild(style);

  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const fmt = n => Number(n || 0).toLocaleString('en-US');
  const blank = (title, text) => `<div class="blank"><b>${esc(title)}</b><span>${text}</span></div>`;
  const STATE = new Map();   // list id -> { top, key, idx }: what a redraw of the page puts back

  function mount(host, o) {
    const id = o.id || ('vl' + Math.random());
    const st = STATE.get(id) || { top: 0, key: null, idx: 0 };
    STATE.set(id, st);
    const OVER = 3;
    let count = 0, first = -1, last = -1, raf = 0, maxRows = o.maxRows || 12, rowH = 28;
    host.classList.add('vl');
    host.tabIndex = 0;
    host.dataset.keep = id;
    host.setAttribute('role', 'listbox');
    if (o.label) host.setAttribute('aria-label', o.label);
    host.innerHTML = '<div class="vl-in"><div class="vl-win"></div></div>';
    const inner = host.firstChild, win = inner.firstChild;
    const rh = () => (typeof o.rowH === 'function' ? o.rowH() : o.rowH) || 28;

    function size() {
      rowH = rh();
      host.style.setProperty('--vh', rowH + 'px');
      inner.style.height = count * rowH + 'px';
      host.style.height = Math.min(count, maxRows) * rowH + 'px';
    }
    function paint(force) {
      const viewH = Math.min(count, maxRows) * rowH;
      const a = Math.max(0, Math.floor(host.scrollTop / rowH) - OVER), b = Math.min(count, Math.ceil((host.scrollTop + viewH) / rowH) + OVER);
      if (!force && a === first && b === last) return;
      first = a; last = b;
      let h = '';
      for (let i = a; i < b; i++) h += o.row(i);
      win.style.top = a * rowH + 'px';
      win.innerHTML = h;
      const kids = win.children;
      for (let j = 0; j < kids.length; j++) {
        const i = a + j, el = kids[j];
        el.dataset.i = i;
        el.setAttribute('role', 'option');
        if (i === 0) el.classList.add('vl-first');
        if (i === st.idx && st.key != null) el.classList.add('kb');
      }
    }
    function keyAt(i) { return i >= 0 && i < count ? String(o.key(i)) : null; }
    function setActive(i, reveal) {
      i = Math.max(0, Math.min(count - 1, i));
      if (!count) { st.key = null; return; }
      st.idx = i; st.key = keyAt(i);
      if (reveal) {
        const top = i * rowH, bot = top + rowH;
        if (top < host.scrollTop) host.scrollTop = top;
        else if (bot > host.scrollTop + host.clientHeight) host.scrollTop = bot - host.clientHeight;
      }
      paint(true);
    }
    // The active row follows its key across a new snapshot; if the key left, the same place in the list stays active.
    function follow() {
      if (st.key == null || !count) { if (!count) st.key = null; return; }
      if (keyAt(st.idx) === st.key) return;
      for (let i = 0; i < count; i++) if (String(o.key(i)) === st.key) { st.idx = i; return; }
      st.idx = Math.min(st.idx, count - 1); st.key = keyAt(st.idx);
    }
    function update(p) {
      if (p) {
        if (p.count != null) count = p.count;
        if (p.maxRows) maxRows = p.maxRows;
        for (const k of ['key', 'row', 'onOpen', 'onKey', 'rowH']) if (p[k]) o[k] = p[k];
      }
      size(); follow();
      host.scrollTop = st.top;
      st.top = host.scrollTop;
      paint(true);
    }
    host.addEventListener('scroll', () => {
      st.top = host.scrollTop;
      if (raf) return;
      raf = requestAnimationFrame(() => { raf = 0; paint(false); });
    }, { passive: true });
    host.addEventListener('click', e => {
      const el = e.target.closest('.vl-win > .row');
      if (!el || !host.contains(el) || e.target.closest('button.ra')) return;   // a row's own button belongs to the page
      const i = +el.dataset.i;
      st.idx = i; st.key = keyAt(i);
      host.focus({ preventScroll: true });
      paint(true);
      if (o.onOpen && !e.target.closest('button, a, input, select, textarea, summary')) o.onOpen(i, e);
    });
    host.addEventListener('keydown', e => {
      if (e.target !== host || e.metaKey || e.ctrlKey || e.altKey) return;
      const cur = st.key == null ? -1 : st.idx;
      if (o.onKey && cur >= 0 && o.onKey(e, cur)) return;
      const page = Math.max(1, Math.floor(host.clientHeight / rowH) - 1);
      const to = { ArrowDown: cur + 1, ArrowUp: cur - 1, PageDown: cur + page, PageUp: cur - page, Home: 0, End: count - 1 }[e.key];
      if (to !== undefined) { e.preventDefault(); setActive(cur < 0 && e.key === 'ArrowUp' ? 0 : to, true); return; }
      if ((e.key === 'Enter' || e.key === ' ') && cur >= 0 && o.onOpen) { e.preventDefault(); o.onOpen(cur, e); }
    });
    host.addEventListener('focus', () => { if (st.key == null && count) setActive(Math.floor(host.scrollTop / rowH), false); else paint(true); });
    host.addEventListener('blur', e => { if (!host.contains(e.relatedTarget)) paint(true); });
    count = o.count || 0;
    update();
    return {
      host, update, redraw: () => { follow(); paint(true); },
      reveal(key) { for (let i = 0; i < count; i++) if (String(o.key(i)) === String(key)) { setActive(i, true); return; } },
      get active() { return st.key; },
    };
  }

  // Runs fn, a redraw that replaces HTML, and puts back the focused field with its caret and the page's scroll.
  function preserve(fn) {
    const a = document.activeElement, keep = a && a.dataset ? a.dataset.keep : null;
    const sel = a && keep && 'selectionStart' in a && a.selectionStart != null ? [a.selectionStart, a.selectionEnd] : null;
    const y = window.scrollY;
    fn();
    if (keep) {
      const el = document.querySelector('[data-keep="' + keep.replace(/["\\]/g, '\\$&') + '"]');
      if (el && el !== document.activeElement) { el.focus({ preventScroll: true }); if (sel && 'setSelectionRange' in el) try { el.setSelectionRange(sel[0], sel[1]); } catch (e) {} }
    }
    if (window.scrollY !== y) window.scrollTo(0, y);
  }

  function sheet(opts) {
    opts = opts || {};
    const el = document.createElement('div');
    el.className = 'sheet-back';
    el.hidden = true;
    el.innerHTML = '<section class="sheet" role="dialog" aria-modal="true"><header class="sheet-head"><span class="sheet-l"></span><span class="sheet-r"><span class="sheet-nav"></span><button data-close title="Close (Esc)">Close</button></span></header><div class="sheet-body" tabindex="0"></div><footer class="sheet-acts"></footer></section>';
    document.body.appendChild(el);
    const card = el.firstChild, L = card.querySelector('.sheet-l'), NAV = card.querySelector('.sheet-nav'), BODY = card.querySelector('.sheet-body'), ACTS = card.querySelector('.sheet-acts');
    let opener = null, last = { head: null, nav: null, body: null, acts: null };
    const isOpen = () => !el.hidden;
    function close(silent) {
      if (el.hidden) return;
      el.hidden = true;
      last = { head: null, nav: null, body: null, acts: null };
      const back = opener && opener.isConnected ? opener : (opener && opener.dataset && opener.dataset.keep ? document.querySelector('[data-keep="' + opener.dataset.keep.replace(/["\\]/g, '\\$&') + '"]') : null);
      opener = null;
      if (back) back.focus({ preventScroll: true });
      if (!silent && opts.onClose) opts.onClose();
    }
    // Updates the parts that changed; the body keeps its scroll position when its HTML is replaced.
    function set(p) {
      if (el.hidden) { opener = document.activeElement && document.activeElement !== document.body ? document.activeElement : null; el.hidden = false; BODY.focus({ preventScroll: true }); }
      if (p.head != null && p.head !== last.head) { L.innerHTML = p.head; last.head = p.head; }
      if (p.nav != null && p.nav !== last.nav) { NAV.innerHTML = p.nav; last.nav = p.nav; }
      if (p.acts != null && p.acts !== last.acts) { ACTS.innerHTML = p.acts; last.acts = p.acts; }
      if (p.body != null && p.body !== last.body) {
        const top = p.reset ? 0 : BODY.scrollTop;
        BODY.innerHTML = p.body; last.body = p.body;
        if (p.mount) p.mount(BODY);
        BODY.scrollTop = top;
      }
      if (p.reset) BODY.scrollTop = 0;
    }
    el.addEventListener('mousedown', e => { if (e.target === el) el._down = true; else el._down = false; });
    el.addEventListener('click', e => { if (e.target === el && el._down || e.target.closest('[data-close]')) close(); });
    document.addEventListener('keydown', e => {
      if (el.hidden) return;
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); return; }
      if (e.key !== 'Tab') return;
      const f = [...card.querySelectorAll('button:not(:disabled), a[href], input:not(:disabled), select, textarea, [tabindex="0"]')].filter(x => x.offsetParent !== null);
      if (!f.length) { e.preventDefault(); return; }
      const i = f.indexOf(document.activeElement);
      if (e.shiftKey && (i <= 0)) { e.preventDefault(); f[f.length - 1].focus(); }
      else if (!e.shiftKey && (i === f.length - 1 || !card.contains(document.activeElement))) { e.preventDefault(); f[0].focus(); }
    }, true);
    return { el, card, body: BODY, set, close, isOpen };
  }

  // Lists a page builds into an HTML string: host() returns the empty div and remembers the list, mountAll(root) fills every
  // remembered host once the HTML is in the page. A list longer than its cap shows `cap` rows and offers Show more, which
  // shows TALL rows; bindExpand(redraw) wires the button.
  const pending = [], expanded = {}, lists = {}, TALL = 22;
  function host(id, items, row, onOpen, cap, o) { pending.push({ id, items, row, onOpen, cap: cap || 10, o: o || {} }); return `<div data-g="${esc(id)}"></div>`; }
  function mountAll(root) {
    for (const m of pending.splice(0)) {
      const el = root.querySelector(`[data-g="${CSS.escape(m.id)}"]`);
      if (!el) continue;
      lists[m.id] = mount(el, { id: m.id, count: m.items.length, rowH: m.o.rowH || 28, maxRows: Math.min(m.items.length, expanded[m.id] ? TALL : m.cap), label: m.o.label || m.id.split(':')[0],
        key: i => m.items[i].id != null ? m.items[i].id : m.items[i].key, row: i => m.row(m.items[i], i), onOpen: m.onOpen ? i => m.onOpen(m.items[i], i) : null });
    }
  }
  const more = (id, n, cap) => n > (cap || 10) ? `<button class="linkbtn" data-exp="${esc(id)}">${expanded[id] ? 'Show fewer' : 'Show more'}</button>` : '';
  const hint = (text, id, n, cap) => `<span class="hint"><span class="cnt">${text || ''}</span>${more(id, n, cap)}</span>`;
  const bindExpand = redraw => document.addEventListener('click', e => { const b = e.target.closest('[data-exp]'); if (b) { expanded[b.dataset.exp] = !expanded[b.dataset.exp]; redraw(); } });
  const drop = () => { pending.length = 0; };
  g.VList = { mount, sheet, preserve, fmt, esc, blank, host, mountAll, more, hint, bindExpand, drop, lists, expanded, TALL };
})(window);
