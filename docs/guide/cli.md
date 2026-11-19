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

## Reporting what tsox does not check

When tsox meets something it does not check yet, it prints a `TSR9xxx` warning and carries on. These flags turn those warnings into a report:

| Flag | What it does |
| --- | --- |
| `--report[=FILE]` | Writes `FILE` (default `tsox-report.md`) with every such place, with file names and lines. Stays on your machine. |
| `--report-issue` | Prints, and opens, a link to a prefilled GitHub issue in `tsoxide/tsox`. Nothing is sent until you press submit on GitHub. |
| `--report-submit` | Shows the exact issue text, asks `y/N` (or pass `--yes`), then posts it with the GitHub CLI (`gh`). Falls back to the link. |
| `--report-names` | Keep quoted names in the issue text (by default `'Foo'` becomes `'_'`). |
| `--no-open` | Print the link but do not open a browser. |

The issue text holds codes, counts, messages, the tsox version and the OS. It never holds paths, line numbers or source code. Set `TSOX_REPORT_REPO` to send it to a fork instead.
