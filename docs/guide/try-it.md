# Try a build on your own machine

Every run of the **Release** workflow leaves downloadable files. Nothing needs to be published
for you to test them.

## 1. Get the files

On GitHub: **Actions → Release → (a run) → Artifacts**. Run it from **Run workflow** to build the
current commit. You get:

| Artifact | Contents |
| --- | --- |
| `tsox-x86_64-unknown-linux-gnu` | `tsox`, `tsox-lsp` for Linux (x86_64), as `.tar.gz` |
| `tsox-x86_64-pc-windows-msvc` | `tsox.exe`, `tsox-lsp.exe` for Windows (x86_64), as `.zip` |
| `tsox-npm` | `tsox-<version>.tgz`, the WebAssembly package |
| `tsox-vscode-linux-x64`, `tsox-vscode-win32-x64` | `.vsix` with the server inside |

With the GitHub CLI: `gh run download <run-id>`.

## 2. Command line

Linux:

```sh
tar xzf tsox-*-x86_64-unknown-linux-gnu.tar.gz
cd tsox-*-x86_64-unknown-linux-gnu
./tsox --version
./tsox --project /path/to/your/project/tsconfig.json
```

Windows (PowerShell):

```powershell
Expand-Archive tsox-*-x86_64-pc-windows-msvc.zip -DestinationPath .
cd tsox-*-x86_64-pc-windows-msvc
.\tsox.exe --version
.\tsox.exe --project C:\path\to\project\tsconfig.json
```

Windows may warn about an unsigned program the first time: **More info → Run anyway**.

## 3. WebAssembly package

```sh
mkdir try-tsox && cd try-tsox && npm init -y
npm install /path/to/tsox-0.1.0.tgz
node -e 'const {TsoxChecker}=require("tsox"); console.log(new TsoxChecker().checkSource("let a: string = 1;","a.ts"))'
```

It prints one diagnostic, `Type 'number' is not assignable to type 'string'.`, at line 1,
column 5 (positions are one-based).

## 4. VS Code

**Extensions → … → Install from VSIX…**, pick the file for your platform, open a folder with a
`tsconfig.json`, and set `"typescript.validate.enable": false` for that workspace so you see only
tsox's diagnostics. **tsox: Show Output** shows the server's log.

## Building the WebAssembly package yourself

You need the `wasm32-unknown-unknown` standard library and a `wasm-bindgen-cli` that matches
`Cargo.lock`. With rustup: `rustup target add wasm32-unknown-unknown`. With conda-forge:

```sh
conda install -c conda-forge rust-std-wasm32-unknown-unknown=<your rustc version>
conda install -c conda-forge wasm-bindgen-cli=<version in Cargo.lock>
bash scripts/build-wasm.sh && node npm/tests/smoke.js
```

## Publishing

`scripts/publish-npm.sh` builds, tests and does a dry run. `scripts/publish-npm.sh --publish`
publishes, reading `NPM_TOKEN` from the environment or `.env.development`
(copy `.env.development.example`). In CI, add the `NPM_TOKEN` repository secret; the Release
workflow publishes on a `v*` tag, or from **Run workflow** with *publish_npm* ticked.
