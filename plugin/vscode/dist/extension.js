"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const path = __importStar(require("path"));
const vscode_1 = require("vscode");
const node_1 = require("vscode-languageclient/node");
let client;
function activate(context) {
    // Resolve the path to the ctx-lsp binary.
    // The extension lives under plugin/vscode/, so the project root is two levels up.
    const extensionRoot = context.extensionPath; // .../plugin/vscode/
    const projectRoot = path.resolve(extensionRoot, "../../.."); // .../context-tag/
    const releasePath = path.join(projectRoot, "target", "release", "ctx-lsp");
    const debugPath = path.join(projectRoot, "target", "debug", "ctx-lsp");
    // Prefer the release binary, fall back to debug.
    let serverCommand;
    try {
        require("fs").accessSync(releasePath);
        serverCommand = releasePath;
    }
    catch {
        serverCommand = debugPath;
    }
    const serverModule = serverCommand;
    const serverOptions = {
        run: { command: serverModule, transport: node_1.TransportKind.stdio },
        debug: { command: serverModule, transport: node_1.TransportKind.stdio },
    };
    const clientOptions = {
        documentSelector: [
            { scheme: "file", language: "c" },
            { scheme: "file", language: "cpp" },
        ],
        synchronize: {
            // Synchronize setting changes to the server
            configurationSection: ["ctxtag"],
        },
        initializationOptions: {
            rootPath: vscode_1.workspace.workspaceFolders?.[0]?.uri.fsPath,
        },
    };
    client = new node_1.LanguageClient("ctxLsp", "Context Tag Language Server", serverOptions, clientOptions);
    client.start();
}
function deactivate() {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
//# sourceMappingURL=extension.js.map