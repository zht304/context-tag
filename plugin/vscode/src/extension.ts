import * as path from "path";
import { workspace, ExtensionContext } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient;

export function activate(context: ExtensionContext) {
  // Resolve the path to the ctx-lsp binary.
  // The extension lives under plugin/vscode/, so the project root is two levels up.
  const extensionRoot = context.extensionPath; // .../plugin/vscode/
  const projectRoot = path.resolve(extensionRoot, "../../.."); // .../context-tag/

  const releasePath = path.join(projectRoot, "target", "release", "ctx-lsp");
  const debugPath = path.join(projectRoot, "target", "debug", "ctx-lsp");

  // Prefer the release binary, fall back to debug.
  let serverCommand: string;
  try {
    require("fs").accessSync(releasePath);
    serverCommand = releasePath;
  } catch {
    serverCommand = debugPath;
  }

  const serverModule = serverCommand;

  const serverOptions: ServerOptions = {
    run: { command: serverModule, transport: TransportKind.stdio },
    debug: { command: serverModule, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "c" },
      { scheme: "file", language: "cpp" },
    ],
    synchronize: {
      // Synchronize setting changes to the server
      configurationSection: ["ctxtag"],
    },
    initializationOptions: {
      rootPath: workspace.workspaceFolders?.[0]?.uri.fsPath,
    },
  };

  client = new LanguageClient(
    "ctxLsp",
    "Context Tag Language Server",
    serverOptions,
    clientOptions
  );

  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
