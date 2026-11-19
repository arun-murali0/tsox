// The ten workstreams of the remaining-work register (architecture/remaining-work-gap-register.md)
// with the state recorded in architecture/new-architecture-progress.md. The register holds the
// design; this file holds the one-line status that the plan page shows. VitePress puts a leading
// underscore on a heading id that starts with a digit, so each anchor below does too.

export type State = "done" | "partial" | "open";

export interface Workstream {
  id: string;
  title: string;
  why: string;
  size: string;
  priority: string;
  state: State;
  done?: string;
  next: string;
  anchor: string;
}

export const workstreams: Workstream[] = [
  {
    id: "W1",
    title: "Generics and inference solver",
    why: "The largest source of call and generic mismatches.",
    size: "Large",
    priority: "High",
    state: "partial",
    done: "Split into infer, instantiate and reduce; named cut reasons; reducer contract for six operators; opt-in inference trace.",
    next: "One Budget object, the solver outcome contract, the tsc priority set, return-type and higher-order inference.",
    anchor: "_3-w1-generics-and-inference-solver",
  },
  {
    id: "W2",
    title: "Options and unused-code diagnostics",
    why: "Common in real projects and cheap to add.",
    size: "Small to medium",
    priority: "High",
    state: "partial",
    done: "Every option is read through an accessor that records the read, so an option no unit reads re-checks nothing.",
    next: "The unused-code pass, then the missing options (isolatedModules, verbatimModuleSyntax and the rest).",
    anchor: "_4-w2-missing-options-and-unused-code-diagnostics",
  },
  {
    id: "W3",
    title: "Thin language areas",
    why: "Decorators, JSX, `using`, `unique symbol`, module augmentation and triple-slash directives are hit by real code.",
    size: "Medium each",
    priority: "Medium",
    state: "open",
    next: "One area at a time: fixtures first, then the check, then a registry entry with an owner.",
    anchor: "_5-w3-thin-language-areas-search",
  },
  {
    id: "W4",
    title: "JavaScript and JSDoc checking",
    why: "A whole class of projects, and the largest item on the list.",
    size: "Very large",
    priority: "Later",
    state: "open",
    next: "Decide whether JS support is in the first public release or a documented later item.",
    anchor: "_6-w4-javascript-and-jsdoc-checking",
  },
  {
    id: "W5",
    title: "Build mode and project references",
    why: "Monorepos and CI.",
    size: "Large",
    priority: "Medium",
    state: "open",
    next: "`tsox build` over the reference graph, with a content-hash up-to-date check. Needs W6 outputs.",
    anchor: "_7-w5-build-mode-and-project-references",
  },
  {
    id: "W6",
    title: "Declaration emit",
    why: "Library authors need `.d.ts` files.",
    size: "Large",
    priority: "Medium",
    state: "open",
    next: "Fast path from explicit annotations first, inferred path second.",
    anchor: "_8-w6-declaration-emit",
  },
  {
    id: "W7",
    title: "Robustness",
    why: "The checker runs on untrusted text in editors, a browser and agents.",
    size: "Medium",
    priority: "High before publishing",
    state: "open",
    done: "A panic inside one unit is already contained in `unit.rs`.",
    next: "Fuzz targets, panic safety for requests, time and memory caps, an audit of every `unsafe` block.",
    anchor: "_9-w7-robustness",
  },
  {
    id: "W8",
    title: "Open-source readiness",
    why: "Contributors cannot join without it.",
    size: "Medium",
    priority: "High before publishing",
    state: "open",
    next: "Licence notices for vendored files, community files, first-task issues, release pipeline, semver policy.",
    anchor: "_10-w8-open-source-readiness",
  },
  {
    id: "W9",
    title: "Conformance dashboard and observability",
    why: "Shows contributors where to help.",
    size: "Small",
    priority: "Medium",
    state: "open",
    next: "A generated conformance page with a trend per commit, and fallback counters shown beside it.",
    anchor: "_11-w9-conformance-dashboard-and-observability",
  },
  {
    id: "W10",
    title: "Diagnostic ownership",
    why: "The owner map has to use tsox's own code namespace.",
    size: "Small",
    priority: "High, early",
    state: "done",
    done: "`cargo xtask owners` generates the map of every code to its message function and raising code, with the 9000s backlog; CI checks it is current.",
    next: "A fixture-per-code column.",
    anchor: "_12-w10-diagnostic-ownership",
  },
];

export interface Round {
  n: number;
  work: string[];
  why: string;
}

// The recommended order, from section 13 of the register.
export const order: Round[] = [
  { n: 1, work: ["W10", "W2"], why: "W10 owner map and the W2 options accessor. Small, and they unblock contributors and incremental checking." },
  { n: 2, work: ["W1", "W2"], why: "W1 steps 1 to 4 (split, budget, registry, trace), then the W2 unused-code pass and missing options. Structure first, then common diagnostics." },
  { n: 3, work: ["W7", "W8"], why: "Fuzzing, panic safety, legal and community files. Required before publishing." },
  { n: 4, work: ["W1", "W9"], why: "W1 steps 5 to 7 (priorities, return-type, higher-order) and the dashboard. Behaviour changes against a tracked baseline." },
  { n: 5, work: ["W3", "W6"], why: "Language areas one at a time, and declaration emit on the fast path." },
  { n: 6, work: ["W5", "W6"], why: "Build mode, and the inferred path of declaration emit. Build mode needs the emit outputs." },
  { n: 7, work: ["W4"], why: "JavaScript and JSDoc. Large, and optional for a first release." },
];
