'use strict';

const users = require('./users');

/** Handles GET /users/:id. */
function handleGetUser(params) {
  const user = users.getUser(Number(params.id));
  if (!user) {
    return { status: 404, body: { error: 'no such user' } };
  }
  return { status: 200, body: user };
}

module.exports = { handleGetUser };
