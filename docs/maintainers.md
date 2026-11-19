# Maintaining the repository

For the owner. None of this is needed to contribute.

## Branch rules on `main`

Run once as the repository admin (needs the GitHub CLI, signed in):

```sh
scripts/protect-main.sh
```

It requires a pull request for every change, the checks **All green** and **Corpus map**, an approving
review from a code owner (`.github/CODEOWNERS` names `@Arun-murali0` for everything), a branch up to
date with `main`, linear history, and no force pushes or deletion.

## Who merges what

| Who | How their change reaches `main` |
| --- | --- |
| `Arun-murali0` | Push a branch. When fmt, layering, tests and the corpus map pass, `auto-merge.yml` squashes it onto `main` through a staging branch, where the fixtures are compared with real tsc. |
| Anyone else | Open a pull request. When the checks pass, the owner reviews and merges. Nothing from other accounts is merged automatically. |

`auto-merge.yml` also merges `main` into every other branch so they stay current; a conflict opens an
issue for a person to resolve.

## Secrets

| Secret | Used by | Purpose |
| --- | --- | --- |
| `AUTO_MERGE_TOKEN` | `auto-merge.yml` | A personal access token of the owner, so pushes made by the workflow start the other workflows. Without it only branch syncing runs. |
| `NPM_TOKEN` | `release.yml` | Publishes the `tsox` npm package on a `v*` tag or a manual run. Locally, `scripts/publish-npm.sh` reads it from `.env.development`. |

## GitHub Pages

Settings → Pages → Source: **GitHub Actions**. The `Docs` workflow builds this site and deploys it
from `main`, at `https://tsoxide.github.io/tsox/`.

## Releasing

1. Update the versions (`Cargo.toml`, `npm/tsox/package.json`, `editors/vscode/package.json`,
   `editors/zed/extension.toml`).
2. Tag: `git tag v0.1.0 && git push origin v0.1.0`.
3. The `Release` workflow builds the Linux and Windows binaries, the WebAssembly package and the
   VS Code extensions, publishes the npm package and creates the GitHub release.
