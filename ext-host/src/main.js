const rpc = require('./rpc');
const extensionLoader = require('./extension-loader');
const path = require('path');

// main entry point
rpc.on('loadExtension', async (params) => {
    try {
        const extData = await extensionLoader.loadExtension(params.path);
        rpc.notify('extensionLoaded', extData);
    } catch (err) {
        rpc.notify('extensionLoadError', { path: params.path, error: err.message });
    }
});

// Setup some dummy API handlers to test connectivity
rpc.notify('hostReady', { pid: process.pid });
