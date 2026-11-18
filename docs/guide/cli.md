# Command line

```sh
tsox --project path/to/tsconfig.json
```

It reads the config, checks the files its `include`, `exclude` and `files` select, and resolves
imports with `paths`, `baseUrl`, `extends` and `references`. Diagnostics go to standard output;
the exit code is non-zero when there are errors.

| Option | Meaning |
| --- | --- |
| `--project <path>` | The `tsconfig.json` to use. |
| `--flat` | Check each file on its own, without the module graph. |
| `--incremental`, `--watch` | Keep the project in memory and re-check on change. |
| `--tsc-codes` | Label diagnostics with the `TSxxxx` code `tsc` uses. |
| `--version`, `-V` | Print the version. |

Run `tsox --help` for the full list.
