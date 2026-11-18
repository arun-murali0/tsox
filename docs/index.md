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
      text: How it works
      link: /architecture
features:
  - title: One engine, three front ends
    details: The same checker runs in the command line tool, the language server and the browser build.
  - title: Incremental
    details: A query layer re-checks only the files an edit can affect, and only the files that read an option you change.
  - title: Measured against tsc
    details: Conformance is compared with TypeScript's own results. Gaps are tracked in the open.
---
