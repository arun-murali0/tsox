# Reporting problems

tsox wants your reports, especially where it disagrees with `tsc`.

Open an issue at <https://github.com/tsoxide/tsox/issues> with:

1. the smallest source that shows it (one file if you can),
2. what `tsc` says and what tsox says,
3. your `tsconfig.json` options if they matter,
4. the output of `tsox --version`.

A report of a crash or a hang is worth as much as a wrong diagnostic. Questions and opinions on
direction are welcome in discussions.

## Report what tsox skipped, from the CLI

Run `tsox --project tsconfig.json --report-issue` and tsox lists every place it did not check (the `TSR9xxx` warnings), then opens a prefilled issue in `tsoxide/tsox`. The text has codes and counts only: no paths, lines or source. `--report` writes a local file with the full sites, and `--report-submit` posts through `gh` after you confirm. See the [CLI guide](./cli.md).
