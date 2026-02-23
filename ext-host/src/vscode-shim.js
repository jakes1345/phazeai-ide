const rpc = require('./rpc');

// This module represents the fake 'vscode' API surface.
// Extensions will require('vscode') and we must intercept it.

const vscode = {
    window: {
        showInformationMessage: async (message, ...items) => {
            return await rpc.call('window.showInformationMessage', { message, items });
        },
        showErrorMessage: async (message, ...items) => {
            return await rpc.call('window.showErrorMessage', { message, items });
        },
        // add more as needed
    },
    commands: {
        registerCommand: (commandId, callback) => {
            rpc.registerCommandHandler(commandId, callback);
            return { dispose: () => rpc.unregisterCommandHandler(commandId) };
        },
        executeCommand: async (commandId, ...args) => {
            return await rpc.call('commands.executeCommand', { commandId, args });
        }
    },
    workspace: {
        getConfiguration: (section) => {
            // Placeholder: real implementation would ask Rust IDE for config
            return {
                get: (key, defaultValue) => defaultValue
            };
        }
    },
    ExtensionContext: class {
        constructor(extensionPath) {
            this.extensionPath = extensionPath;
            this.subscriptions = [];
        }
    }
};

module.exports = vscode;
