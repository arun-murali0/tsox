# Project Configuration: Making `tsconfig.json` an Input to the Graph

> **Architecture case study:** why reading `tsconfig.json` is a matter of correctness and not of convenience, how it selects files and steers resolution, and how a missing package is reported only when the graph is sure.

[module-resolution](module-resolution.md) resolves imports with `oxc_resolver`. This document is about what the project's configuration adds: which files are in the project, how a specifier is mapped to a file, which conditions of a package's `exports` apply, and when the checker may say a package is missing.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-21 | `tsconfig.json` applied to file selection and to module resolution | `bcba50e` | Without it the engine resolves the wrong file, or suppresses a module that is really missing |
| 2026-10-23 | Bare-import reporting, a symlink walk loop and a cancelled second pass fixed | `3d8608d` | Follow-up fixes to the same work |
| 2026-10-24 | The module format follows the nearest `package.json`; tsconfig handling hardened | `4e37ccd` | NodeNext reads a different `exports` condition for a CommonJS file and an ES module |
| 2026-10-31 | Compiler options, type roots and reference directives are read | `35e4962` | The checker needs the options, and `types` and `typeRoots` feed ambient declarations |

## 2. Why this is correctness

**Problem.** Configuration looks like a convenience: a way to pick files and set options. It is not. If the engine resolves the wrong file, or suppresses a genuinely missing module, then type checking can be implemented perfectly and still give an incorrect project result. A path alias that does not resolve produces a false "cannot find module"; a missing package that is silently allowed produces no error where there should be one.

**Picked.** Treat the configuration as a graph input: read it, record which files were read, and let the same graph answer both "what is in the project" and "what does this specifier mean".

## 3. What is built

**Configuration (`crates/tsox-project/src/module_resolution/config.rs`).** `ProjectConfig::load` reads a config as JSON with comments and trailing commas, follows `extends` (a path, a package, or a list), merges `compilerOptions` key by key with the extending file winning, and resolves `include`, `exclude` and `files` relative to the file that wrote them. With no `include` or `files`, everything under the config is selected. `node_modules`, dot-directories and the `outDir` are left out by default, and a `.ts` file hides the `.tsx` and `.d.ts` of the same name. A config that cannot be read, or an `extends` that names nothing or loops, is an error and not an empty project.

**Resolution.** `ModuleResolver::with_tsconfig` hands the config to `oxc_resolver`, which applies `paths` (with and without `baseUrl`), `baseUrl` alone, `extends` chains, project `references` and `rootDirs`. A specifier the config does not map resolves as before, and an alias that points at nothing is still unresolved.

**`exports` conditions follow the importing file.** An `import` statement reads the `import` condition; `import x = require()` and any import in a `.cts` file read `require`; a `.mts` file reads `import` (`ResolveMode`).

**NodeNext follows the nearest `package.json`.** When `moduleResolution` is `node16` or `nodenext`, or is unset and `module` is, `ProjectConfig::uses_node_next` is true and the CLI builds the resolver with `with_node_next`. A `.ts`, `.tsx`, `.js` or `.jsx` file then reads the `require` condition unless the nearest `package.json` says `"type": "module"`. `.mts`/`.mjs` and `.cts`/`.cjs` keep their extension rule, a dynamic `import()` always reads `import`, and `bundler`, `node` and unset options ignore the package type. The nearest-package lookup is cached per directory.

**The config is a graph input.** `ModuleGraph::with_config` records every config file that was read, and `config_changed()` says one has changed or gone. In incremental mode a config change rebuilds the graph and moves the resolution epoch (`a_config_change_rebuilds_and_applies_the_option`, see [query-layer](query-layer.md)); in the full driver the caller rebuilds.

**Options the checker reads.** `CheckOptions` (in `tsox-base`) carries `noUncheckedIndexedAccess`, `allowUnreachableCode` and `useUnknownInCatchVariables` from the tsconfig (`tests/check_options.rs`). A key left out keeps the checker's old fixed behaviour.

**The command line.** `--project` loads the config, checks the files it selects, uses its directory as the root, accepts a bare file name such as `tsconfig.json`, and falls back to every file, with a warning, when the config cannot be read.

## 4. When a package may be called missing

**Problem.** A project that imports a package nobody installed should get TS2307. But the checker cannot always know. If the configuration could map the specifier somewhere the checker did not look, then reporting it missing is a false error.

**Picked.** A gate that decides, from the effective configuration, whether a bare specifier that does not resolve may be reported. `ModuleGraph::config_hides_bare_specifiers` decides after `extends`: only `typeRoots` can still hide a missing package, plus `paths` or `baseUrl` when the resolver was not given the config. `rootDirs` does not, because it only merges directories for relative imports. With `types` and `typeRoots` now applied, an `@types` directory or a `typeRoots` setting no longer holds the report back. A missing package that nothing answers is a TS2307 even in a project with path aliases.

**Cost.** Without a parsed config the gate falls back to searching the config text, and there it still counts `rootDirs`, `extends`, `typeRoots` and, when the resolver was not given the config, `paths` and `baseUrl`. That is deliberately cautious: it reports less, never more.

## 5. Evidence

`tests/tsconfig_resolution.rs`, `tests/project_config.rs` (including the NodeNext cases) and `crates/tsox-cli/tests/cli_project.rs`. The exit condition was that projects using the supported path aliases and package `exports` resolve correctly with no false TS2307, and that a configuration change produces the expected graph update. Both hold.

## 6. What is still not done

- Other effects of `moduleResolution`: the `node10` and `classic` rule sets, extension rules for relative imports under NodeNext, and `customConditions`. The setting is read only to switch NodeNext on.
- Compiler options other than the three above are kept as written and not read by the checker.
- Automatic graph updates on a config change in the full driver. Incremental mode has them.

## 7. Invariants to keep

1. **The config is an input, not a side effect.** Every config file that was read is recorded, and a change to one is detected.
2. **A path that does not resolve is unresolved.** An alias pointing at nothing is never turned into a guess.
3. **Missing means sure.** TS2307 is reported only when the graph is sure the configuration could not have supplied the package.
4. **Less, never more.** Where the gate cannot tell, it reports nothing.
