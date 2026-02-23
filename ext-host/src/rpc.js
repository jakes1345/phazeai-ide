// Handles JSON-RPC 2.0 over stdin/stdout

const EventEmitter = require('events');

class RpcManager extends EventEmitter {
    constructor() {
        super();
        this.nextId = 1;
        this.pendingRequests = new Map();
        this.commandHandlers = new Map();

        let buffer = '';
        process.stdin.on('data', chunk => {
            buffer += chunk.toString();
            let newlineIdx;
            while ((newlineIdx = buffer.indexOf('\n')) >= 0) {
                const line = buffer.slice(0, newlineIdx).trim();
                buffer = buffer.slice(newlineIdx + 1);
                if (line) {
                    this.handleMessage(line);
                }
            }
        });
    }

    send(message) {
        process.stdout.write(JSON.stringify(message) + '\n');
    }

    call(method, params) {
        return new Promise((resolve, reject) => {
            const id = this.nextId++;
            this.pendingRequests.set(id, { resolve, reject });
            this.send({
                jsonrpc: '2.0',
                id,
                method,
                params
            });
        });
    }

    notify(method, params) {
        this.send({
            jsonrpc: '2.0',
            method,
            params
        });
    }

    async handleMessage(line) {
        try {
            const msg = JSON.parse(line);
            if (msg.id !== undefined && msg.method) {
                // Incoming request
                try {
                    const result = await this.handleRequest(msg.method, msg.params);
                    this.send({ jsonrpc: '2.0', id: msg.id, result });
                } catch (err) {
                    this.send({ jsonrpc: '2.0', id: msg.id, error: { code: -32000, message: err.message } });
                }
            } else if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
                // Incoming response
                const pending = this.pendingRequests.get(msg.id);
                if (pending) {
                    this.pendingRequests.delete(msg.id);
                    if (msg.error) pending.reject(new Error(msg.error.message));
                    else pending.resolve(msg.result);
                }
            } else if (msg.method) {
                // Incoming notification
                this.emit(msg.method, msg.params);
            }
        } catch (e) {
            // Ignore parse errors
        }
    }

    async handleRequest(method, params) {
        if (method === 'executeCommand') {
            const { commandId, args } = params;
            const handler = this.commandHandlers.get(commandId);
            if (handler) {
                return await handler(...(args || []));
            } else {
                throw new Error(`Command not found: ${commandId}`);
            }
        }
        throw new Error(`Method not found: ${method}`);
    }

    registerCommandHandler(commandId, callback) {
        this.commandHandlers.set(commandId, callback);
        this.notify('commands.register', { commandId });
    }

    unregisterCommandHandler(commandId) {
        this.commandHandlers.delete(commandId);
        this.notify('commands.unregister', { commandId });
    }
}

module.exports = new RpcManager();
