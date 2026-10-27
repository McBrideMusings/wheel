// Run: node artifacts/beads/query.test.js
const assert = require('assert');
const { parse } = require('./query.js');

const bead = (id, o) => ({ id, title: id, status: 'open', type: 'task', priority: 2, labels: [], owner: null, blocked_by: [], ready: false, ...o });
const fixture = [
  bead('b1', { type: 'bug', status: 'open', labels: ['mac'], priority: 1, ready: true }),
  bead('b2', { type: 'bug', status: 'closed', labels: ['mac'], priority: 1 }),
  bead('b3', { type: 'bug', status: 'in_progress', labels: ['ios'], priority: 0, owner: 'ada' }),
  bead('b4', { type: 'task', status: 'open', labels: ['ios'], priority: 3, blocked_by: ['b1'] }),
  bead('b5', { type: 'feature', status: 'open', labels: ['daemon'], priority: 2, title: 'Query box' }),
  bead('b6', { type: 'bug', status: 'open', labels: ['cli'], priority: 4 }),
];
const ids = q => fixture.filter(parse(q)).map(b => b.id);
const eq = (q, want) => assert.deepStrictEqual(ids(q), want, q);

eq('type=bug AND NOT status=closed', ['b1', 'b3', 'b6']);
eq('label=mac OR label=ios', ['b1', 'b2', 'b3', 'b4']);
eq('label=mac OR label=ios AND type=bug', ['b1', 'b2', 'b3']);
eq('(label=mac OR label=ios) AND type=bug', ['b1', 'b2', 'b3']);
eq('', ['b1', 'b2', 'b3', 'b4', 'b5', 'b6']);
eq('priority<=1', ['b1', 'b2', 'b3']);
eq('priority>P2', ['b4', 'b6']);
eq('priority!=1', ['b3', 'b4', 'b5', 'b6']);
eq('status=blocked', ['b4']);
eq('status!=open', ['b2', 'b3']);
eq('title="query box"', ['b5']);
eq('assignee=ada', ['b3']);
eq('not type=bug', ['b4', 'b5']);
eq('TYPE=Bug and label=CLI', ['b6']);

for (const bad of ['type=', 'colour=red', 'type=bug AND', '(type=bug', 'type=bug)', 'title>x', 'priority=high', 'type bug', 'AND type=bug', 'title="x']) {
  assert.throws(() => parse(bad), Error, `"${bad}" should not parse`);
}
console.log('query.test.js: all passed');
