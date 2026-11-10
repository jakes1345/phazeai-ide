'use strict';

const { getUsr, allIds } = require('./users');

/** One line per user: "1 Ada (admin)". */
function report() {
  return allIds()
    .map((id) => getUsr(id))
    .map((u) => `${u.id} ${u.name}${u.admin ? ' (admin)' : ''}`)
    .join('\n');
}

module.exports = { report };
