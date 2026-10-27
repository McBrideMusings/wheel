// A bd-style query over trimmed beads (data/beads.json): field=value, !=, >, >=, <, <=, AND, OR, NOT, parentheses.
// parse(text) returns a predicate over a bead, or throws an Error whose message the page shows under the query box.
// An empty query matches every bead.
(function (root) {
  const FIELDS = ['status', 'priority', 'type', 'label', 'title', 'assignee', 'owner'];
  const WORDS = { AND: 'and', OR: 'or', NOT: 'not' };

  function tokenize(text) {
    const toks = [];
    let i = 0;
    while (i < text.length) {
      const c = text[i];
      if (/\s/.test(c)) { i++; continue; }
      if (c === '(' || c === ')') { toks.push({ t: c, at: i }); i++; continue; }
      const two = text.slice(i, i + 2);
      if (two === '!=' || two === '>=' || two === '<=') { toks.push({ t: 'op', v: two, at: i }); i += 2; continue; }
      if (c === '=' || c === '>' || c === '<') { toks.push({ t: 'op', v: c, at: i }); i++; continue; }
      if (c === '"' || c === "'") {
        const end = text.indexOf(c, i + 1);
        if (end < 0) throw new Error(`unterminated quote at column ${i + 1}`);
        toks.push({ t: 'word', v: text.slice(i + 1, end), quoted: true, at: i });
        i = end + 1;
        continue;
      }
      let j = i;
      while (j < text.length && !/[\s()=<>!"']/.test(text[j])) j++;
      if (j === i) throw new Error(`unexpected "${c}" at column ${i + 1}`);
      toks.push({ t: 'word', v: text.slice(i, j), at: i });
      i = j;
    }
    return toks;
  }

  const lower = s => String(s ?? '').toLowerCase();
  const priorityOf = v => {
    const m = /^p?(\d+)$/i.exec(String(v));
    if (!m) throw new Error(`priority wants a number such as 1 or P1, not "${v}"`);
    return Number(m[1]);
  };

  function comparison(field, op, value) {
    if (field === 'priority') {
      const want = priorityOf(value);
      return b => {
        if (b.priority == null) return op === '!=';
        const have = Number(b.priority);
        return op === '=' ? have === want : op === '!=' ? have !== want : op === '>' ? have > want
          : op === '>=' ? have >= want : op === '<' ? have < want : have <= want;
      };
    }
    if (op !== '=' && op !== '!=') throw new Error(`${field} takes = or !=, not ${op}`);
    const want = lower(value);
    let has;
    if (field === 'label') has = b => (b.labels || []).some(l => lower(l) === want);
    else if (field === 'title') has = b => lower(b.title).includes(want);
    else if (field === 'type') has = b => lower(b.type) === want;
    else if (field === 'status') has = b => lower(b.status) === want || (want === 'blocked' && b.status === 'open' && !b.ready && (b.blocked_by || []).length > 0);
    else if (field === 'assignee' || field === 'owner') has = b => lower(b.owner) === want;
    return op === '=' ? has : b => !has(b);
  }

  function parse(text) {
    const toks = tokenize(text);
    if (!toks.length) return () => true;
    let p = 0;
    const peek = () => toks[p];
    const isWord = (tok, w) => tok && tok.t === 'word' && !tok.quoted && tok.v.toUpperCase() === w;
    const describe = tok => tok ? `"${tok.v ?? tok.t}" at column ${tok.at + 1}` : 'the end of the query';

    function or() {
      let left = and();
      while (isWord(peek(), 'OR')) { p++; const l = left, r = and(); left = b => l(b) || r(b); }
      return left;
    }
    function and() {
      let left = not();
      while (isWord(peek(), 'AND')) { p++; const l = left, r = not(); left = b => l(b) && r(b); }
      return left;
    }
    function not() {
      if (isWord(peek(), 'NOT')) { p++; const x = not(); return b => !x(b); }
      return atom();
    }
    function atom() {
      const tok = peek();
      if (!tok) throw new Error('the query ends where a condition should be');
      if (tok.t === '(') {
        p++;
        const x = or();
        if (!peek() || peek().t !== ')') throw new Error('missing ")" to close "("');
        p++;
        return x;
      }
      if (tok.t !== 'word' || (!tok.quoted && WORDS[tok.v.toUpperCase()])) throw new Error(`expected a field name, found ${describe(tok)}`);
      const field = tok.v.toLowerCase();
      if (!FIELDS.includes(field)) throw new Error(`unknown field "${tok.v}"; the fields are ${FIELDS.join(', ')}`);
      p++;
      const op = peek();
      if (!op || op.t !== 'op') throw new Error(`${field} needs an operator (=, !=, >, >=, <, <=), found ${describe(op)}`);
      p++;
      const val = peek();
      if (!val || val.t !== 'word') throw new Error(`${field}${op.v} needs a value, found ${describe(val)}`);
      p++;
      return comparison(field, op.v, val.v);
    }

    const result = or();
    if (p < toks.length) throw new Error(`unexpected ${describe(toks[p])}`);
    return result;
  }

  const api = { parse, FIELDS };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.beadQuery = api;
})(typeof window !== 'undefined' ? window : globalThis);
