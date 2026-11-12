'use strict';

const USERS = [
  { id: 1, name: 'Ada', admin: true },
  { id: 2, name: 'Grace', admin: false },
  { id: 3, name: 'Linus', admin: false },
];

/** The user with this id, or null. */
function getUsr(id) {
  return USERS.find((u) => u.id === id) || null;
}

function allIds() {
  return USERS.map((u) => u.id);
}

module.exports = { getUsr, allIds };
