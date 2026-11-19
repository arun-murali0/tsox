---
layout: home
hero:
  name: tsox
  text: TypeScript semantics, in Rust
  tagline: A type checker you can run as a command, a language server, or WebAssembly. Early, honest, and open to your reports.
  actions:
    - theme: brand
      text: Install
      link: /guide/install
    - theme: alt
      text: How it was built
      link: /journey
    - theme: alt
      text: How it works
      link: /architecture
features:
  - title: One engine, three front ends
    details: The same checker runs in the command line tool, the language server and the browser build.
  - title: Incremental
    details: A query layer re-checks only the files an edit can affect, and only the files that read an option you change.
  - title: Measured against tsc
    details: Conformance is compared with TypeScript's own results. Gaps are tracked in the open.
  - title: Built step by step
    details: Every capability has its own tests and a case study of what was planned, what went wrong and what changed.
---

## How it fits together

Oxc parses and binds. tsox owns everything after that.

<Flow :steps="[
  { title: 'Oxc', note: 'Parser and semantic information' },
  { title: 'Bridge', note: 'Behaviour that depends on the AST' },
  { title: 'Semantic layer', note: 'Type arena, subtyping, generics, narrowing' },
  { title: 'Diagnostics and facts', note: 'What the checker found and knows' },
  { title: 'Tooling', note: 'Command line, language server, WebAssembly' },
]" />

The reasoning behind this split is in [architecture](architecture.md).

## Where it stands

How the checker does on 98 small programs, each with one deliberate fault, compared with `tsc`.

<Stats
  :items="[
    { value: 70, label: 'match tsc', tone: 'good' },
    { value: 20, label: 'quiet gaps', tone: 'warn' },
    { value: 5, label: 'different codes', tone: 'warn' },
    { value: 3, label: 'false positives', tone: 'bad' },
  ]"
  caption="Measured on 2026-11-04 against tsc 6.0.3. A false positive is an error tsc does not report."
/>

The probes, the gaps and how the numbers are produced are in
[language coverage](language-coverage.md). Results on the compiler's own tests are in the
[conformance baseline](conformance/baseline-part01-05.md).

## Read next

- **Try it:** [install](guide/install.md), then the [command line](guide/cli.md).
- **See how it was made:** [how tsox was built](journey.md), step by step.
- **See what is left:** the [work plan](plan.md), W1 to W10.
- **Help:** [contributing](contributing.md) and [reporting problems](guide/feedback.md).
