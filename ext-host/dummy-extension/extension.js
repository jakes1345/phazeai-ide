const vscode = require('vscode');

function activate(context) {
    let disposable = vscode.commands.registerCommand('dummy.helloWorld', () => {
        vscode.window.showInformationMessage('Hello from Dummy Extension inside PhazeAI!');
    });

    context.subscriptions.push(disposable);

    // Auto-trigger a message immediately for testing
    vscode.window.showInformationMessage('Dummy extension successfully loaded and activated on Node.js Boot!');
}

function deactivate() { }

module.exports = {
    activate,
    deactivate
}
