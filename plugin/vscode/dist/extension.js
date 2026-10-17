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
const fs = __importStar(require("fs"));
const path = __importStar(require("path"));
const vscode_1 = require("vscode");
const node_1 = require("vscode-languageclient/node");
let client;
/**
 * Locate the ctx-lsp binary.
 * Priority: ctxtag.serverPath setting → repo target/release → repo target/debug → PATH.
 */
function resolveServerPath(extensionRoot) {
    const configured = vscode_1.workspace
        .getConfiguration("ctxtag")
        .get("serverPath", "")
        .trim();
    if (configured) {
        return fs.existsSync(configured) ? configured : undefined;
    }
    // The extension lives at <repo>/plugin/vscode, so the repo root is two levels up.
    const projectRoot = path.resolve(extensionRoot, "../..");
    const candidates = [
        path.join(projectRoot, "target", "release", "ctx-lsp"),
        path.join(projectRoot, "target", "debug", "ctx-lsp"),
    ];
    for (const c of candidates) {
        if (fs.existsSync(c)) {
            return c;
        }
    }
    // Fall back to whatever is on PATH.
    return "ctx-lsp";
}
function activate(context) {
    const serverCommand = resolveServerPath(context.extensionPath);
    if (!serverCommand) {
        vscode_1.window.showErrorMessage("Context Tag: ctx-lsp binary not found. Build it with `cargo build --release`, " +
            "or set ctxtag.serverPath to its absolute path.");
        return;
    }
    const serverOptions = {
        run: { command: serverCommand, transport: node_1.TransportKind.stdio },
        debug: { command: serverCommand, transport: node_1.TransportKind.stdio },
    };
    // Optional: force the workspace root used to find {root}/.ctxtag.db.
    const rootOverride = vscode_1.workspace
        .getConfiguration("ctxtag")
        .get("rootUri", "")
        .trim();
    const clientOptions = {
        documentSelector: [
            { scheme: "file", language: "c" },
            { scheme: "file", language: "cpp" },
        ],
        synchronize: {
            configurationSection: ["ctxtag"],
        },
        traceOutputChannel: vscode_1.workspace.getConfiguration("ctxtag").get("trace.server", "off") !==
            "off"
            ? vscode_1.window.createOutputChannel("Context Tag LSP Trace")
            : undefined,
    };
    if (rootOverride) {
        const fsPath = vscode_1.Uri.parse(rootOverride).fsPath || rootOverride;
        const folder = {
            uri: vscode_1.Uri.file(fsPath),
            name: path.basename(fsPath),
            index: 0,
        };
        clientOptions.workspaceFolder = folder;
    }
    client = new node_1.LanguageClient("ctxLsp", "Context Tag Language Server", serverOptions, clientOptions);
    client.start().catch((err) => {
        vscode_1.window.showErrorMessage(`Context Tag: failed to start ctx-lsp (${serverCommand}): ${err}`);
    });
}
function deactivate() {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
//# sourceMappingURL=extension.js.map