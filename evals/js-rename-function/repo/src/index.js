'use strict';

const { report } = require('./report');

if (require.main === module) {
  console.log(report());
}

module.exports = { report };
