// Run: node artifacts/beads/gh-to-beads.test.js
const assert = require('assert');
const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const { parse } = require('./query.js');

const out = JSON.parse(execFileSync(path.join(__dirname, 'gh-to-beads.sh'), ['acme/importer', path.join(__dirname, 'fixtures/gh-relations.json')],
  { input: fs.readFileSync(path.join(__dirname, 'fixtures/gh-issues.json')) }));
const beads = out.repos[0].beads;
const by = Object.fromEntries(beads.map(b => [b.id, b]));
const count = q => beads.filter(parse(q)).length;

assert.strictEqual(out.repos[0].name, 'acme/importer');
assert.strictEqual(beads.length, 6);
assert.strictEqual(count('status=closed'), 2);
assert.strictEqual(count('status=open'), 4);
assert.strictEqual(count('type=bug'), 1);
assert.strictEqual(count('type=feature'), 1);
assert.strictEqual(count('type=task'), 2);
assert.strictEqual(count('label=importer'), 3);
assert.strictEqual(count('priority=0'), 1);
assert.strictEqual(count('priority=2'), 2);
assert.strictEqual(count('status=blocked'), 1);
assert.deepStrictEqual(by['#5'].blocked_by, ['#3']);
assert.strictEqual(by['#5'].ready, false);
assert.strictEqual(by['#3'].ready, true);
assert.strictEqual(by['#2'].parent, '#1');
assert.strictEqual(by['#1'].epic, true);
assert.strictEqual(by['#6'].type, 'decision');
assert.deepStrictEqual(by['#1'].labels, ['epic', 'P1']);
console.log('gh-to-beads.test.js: all passed');
