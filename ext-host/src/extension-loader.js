const fs = require('fs');
const path = require('path');
const Module = require('module');
const vscode = require('./vscode-shim');

class ExtensionLoader {
    constructor() {
        this.extensions = new Map();

        // Intercept require('vscode')
        const originalRequire = Module.prototype.require;
        Module.prototype.require = function (request) {
            if (request === 'vscode') {
                return vscode;
            }
            return originalRequire.apply(this, arguments);
        };
    }

    async loadExtension(extPath) {
        try {
            const packageJsonPath = path.join(extPath, 'package.json');
            if (!fs.existsSync(packageJsonPath)) {
                throw new Error(`No package.json found in ${extPath}`);
            }

            const pkg = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
            const mainFile = pkg.main || 'index.js';
            const mainPath = path.join(extPath, mainFile);

            if (!fs.existsSync(mainPath)) {
                throw new Error(`Main file ${mainPath} not found`);
            }

            // Load module
            const extModule = require(mainPath);
            const context = new vscode.ExtensionContext(extPath);

            if (typeof extModule.activate === 'function') {
                await extModule.activate(context);
            }

            this.extensions.set(pkg.name, {
                module: extModule,
                context
            });

            return {
                name: pkg.name,
                version: pkg.version
            };
        } catch (err) {
            throw new Error(`Failed to load extension at ${extPath}: ${err.message}`);
        }
    }
}

module.exports = new ExtensionLoader();
