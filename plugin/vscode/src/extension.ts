import * as fs from "fs";
import * as path from "path";
import {
  workspace,
  window,
  Uri,
  WorkspaceFolder,
  ExtensionContext,
} from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

/**
 * Locate the ctx-lsp binary.
 * Priority: ctxtag.serverPath setting → repo target/release → repo target/debug → PATH.
 */
function resolveServerPath(extensionRoot: string): string | undefined {
  const configured = workspace
    .getConfiguration("ctxtag")
    .get<string>("serverPath", "")
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

export function activate(context: ExtensionContext) {
  const serverCommand = resolveServerPath(context.extensionPath);
  if (!serverCommand) {
    window.showErrorMessage(
      "Context Tag: ctx-lsp binary not found. Build it with `cargo build --release`, " +
        "or set ctxtag.serverPath to its absolute path.",
    );
    return;
  }

  const serverOptions: ServerOptions = {
    run: { command: serverCommand, transport: TransportKind.stdio },
    debug: { command: serverCommand, transport: TransportKind.stdio },
  };

  // Optional: force the workspace root used to find {root}/.ctxtag.db.
  const rootOverride = workspace
    .getConfiguration("ctxtag")
    .get<string>("rootUri", "")
    .trim();

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "c" },
      { scheme: "file", language: "cpp" },
    ],
    synchronize: {
      configurationSection: ["ctxtag"],
    },
    traceOutputChannel:
      workspace.getConfiguration("ctxtag").get<string>("trace.server", "off") !==
      "off"
        ? window.createOutputChannel("Context Tag LSP Trace")
        : undefined,
  };

  if (rootOverride) {
    const fsPath = Uri.parse(rootOverride).fsPath || rootOverride;
    const folder: WorkspaceFolder = {
      uri: Uri.file(fsPath),
      name: path.basename(fsPath),
      index: 0,
    };
    clientOptions.workspaceFolder = folder;
  }

  client = new LanguageClient(
    "ctxLsp",
    "Context Tag Language Server",
    serverOptions,
    clientOptions,
  );

  client.start().catch((err) => {
    window.showErrorMessage(
      `Context Tag: failed to start ctx-lsp (${serverCommand}): ${err}`,
    );
  });
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
