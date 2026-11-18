// The tsox VS Code extension: starts `tsox-lsp` over stdio and hands it to the language client.
// It adds no checking of its own. To use tsox next to the built-in TypeScript service, turn the
// built-in diagnostics off for the workspace (`"typescript.validate.enable": false`).
"use strict";

const fs = require("fs");
const path = require("path");
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;
let output;

function exeName() {
  return process.platform === "win32" ? "tsox-lsp.exe" : "tsox-lsp";
}

// The server to run: the setting, then a copy shipped in the extension, then PATH.
function serverCommand(context) {
  const configured = vscode.workspace.getConfiguration("tsox").get("server.path");
  if (configured && configured.trim() !== "") {
    return configured.trim();
  }
  const bundled = path.join(context.extensionPath, "server", exeName());
  if (fs.existsSync(bundled)) {
    return bundled;
  }
  return exeName();
}

async function start(context) {
  const command = serverCommand(context);
  const logLevel = vscode.workspace.getConfiguration("tsox").get("server.logLevel");
  const env = { ...process.env, TSOX_LSP_LOG: logLevel };
  const serverOptions = {
    run: { command, args: ["--stdio"], transport: TransportKind.stdio, options: { env } },
    debug: { command, args: ["--stdio"], transport: TransportKind.stdio, options: { env } },
  };
  const clientOptions = {
    documentSelector: [
      { scheme: "file", language: "typescript" },
      { scheme: "file", language: "typescriptreact" },
      { scheme: "file", language: "javascript" },
      { scheme: "file", language: "javascriptreact" },
    ],
    outputChannel: output,
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/{tsconfig,jsconfig}*.json"),
    },
  };
  client = new LanguageClient("tsox", "tsox", serverOptions, clientOptions);
  try {
    await client.start();
  } catch (error) {
    client = undefined;
    const choice = await vscode.window.showErrorMessage(
      `tsox could not start \`${command}\`: ${error.message}. Install tsox-lsp or set "tsox.server.path".`,
      "Open Settings",
    );
    if (choice === "Open Settings") {
      vscode.commands.executeCommand("workbench.action.openSettings", "tsox.server.path");
    }
  }
}

async function stop() {
  if (client) {
    const running = client;
    client = undefined;
    await running.stop();
  }
}

async function activate(context) {
  output = vscode.window.createOutputChannel("tsox");
  context.subscriptions.push(
    output,
    vscode.commands.registerCommand("tsox.restartServer", async () => {
      await stop();
      await start(context);
    }),
    vscode.commands.registerCommand("tsox.showOutput", () => output.show()),
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration("tsox.server")) {
        await stop();
        await start(context);
      }
    }),
  );
  await start(context);
}

module.exports = { activate, deactivate: stop };
