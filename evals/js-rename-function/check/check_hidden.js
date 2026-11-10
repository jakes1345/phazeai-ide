'use strict';
const assert = require('assert');
const fs = require('fs');
const path = require('path');

const users = require('./src/users');
assert.strictEqual(typeof users.getUser, 'function', 'users.js exports getUser');
assert.strictEqual(users.getUsr, undefined, 'getUsr is gone from the exports');
assert.deepStrictEqual(users.getUser(2), { id: 2, name: 'Grace', admin: false });
assert.strictEqual(users.getUser(99), null);

const { report } = require('./src/report');
assert.strictEqual(report(), '1 Ada (admin)\n2 Grace\n3 Linus');

const { handleGetUser } = require('./src/api');
assert.strictEqual(handleGetUser({ id: '3' }).body.name, 'Linus');
assert.strictEqual(handleGetUser({ id: '7' }).status, 404);

function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(p);
    else if (p.endsWith('.js')) {
      assert(!fs.readFileSync(p, 'utf8').includes('getUsr'), `${p} still says getUsr`);
    }
  }
}
walk(path.join(__dirname, 'src'));
console.log('ok');
