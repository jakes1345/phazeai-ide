const vscode = require('vscode');

function activate(context) {
    vscode.window.showInformationMessage('PhazeAI Test Extension Activated!');

    let disposable = vscode.commands.registerCommand('phazeai.testCommand', function (args) {
        vscode.window.showInformationMessage('Test Command Executed with args: ' + JSON.stringify(args));
        return { success: true, received: args };
    });

    context.subscriptions.push(disposable);
}

function deactivate() {}

module.exports = {
    activate,
    deactivate
}
