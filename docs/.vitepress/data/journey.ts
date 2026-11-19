// The build order of tsox, one entry per milestone, in the order the work was done.
// The prose behind each step lives in its own document; this file only carries the one-line
// summary, the tests that guard it and the link, so the journey page and the README table
// cannot drift apart in structure. Update both when a milestone is added.

export type Status = "done" | "current";

export interface Step {
  n: number;
  title: string;
  purpose: string;
  link: string;
  tests?: string[];
  gate?: string;
  status: Status;
}

export interface Part {
  id: number;
  title: string;
  note: string;
  steps: Step[];
}

export const journey: Part[] = [
  {
    id: 1,
    title: "One file at a time",
    note: "A checker that agrees with tsc on a single file, built on an arena of type ids.",
    steps: [
      {
        n: 1,
        title: "Foundation",
        purpose:
          "The end-to-end checker: primitive types, basic functions, diagnostics, and honest handling of what is not supported yet.",
        link: "/foundation",
        tests: ["tests/foundation.rs"],
        status: "done",
      },
      {
        n: 2,
        title: "Structural Types",
        purpose:
          "Objects, arrays, unions, aliases, interfaces, declaration resolution, structural subtyping and function arity.",
        link: "/structural-types",
        tests: ["tests/structural_types.rs"],
        status: "done",
      },
      {
        n: 3,
        title: "Literal Widening",
        purpose: "Keep literal information while modelling how it widens.",
        link: "/literal-widening",
        tests: ["tests/literal_widening.rs"],
        status: "done",
      },
      {
        n: 4,
        title: "Control-Flow Narrowing",
        purpose: "Refine types across branches and flow paths.",
        link: "/architecture/narrowing",
        tests: ["tests/control_flow_narrowing.rs"],
        status: "done",
      },
      {
        n: 5,
        title: "Classes and Inheritance",
        purpose: "Class members, constructors, inheritance, static state and `this`.",
        link: "/classes-inheritance",
        tests: ["tests/classes_inheritance.rs"],
        status: "done",
      },
      {
        n: 6,
        title: "Expression and Program Checking",
        purpose:
          "Expression inference, calls, members, logical operations, loops, switches, assertions and program-level checking.",
        link: "/expression-program-checking",
        tests: ["tests/expression_program_checking.rs"],
        status: "done",
      },
      {
        n: 7,
        title: "Parameters and Destructuring",
        purpose: "Parameter metadata and binding and destructuring semantics.",
        link: "/parameters-destructuring",
        tests: ["tests/parameters_destructuring.rs"],
        status: "done",
      },
    ],
  },
  {
    id: 2,
    title: "Generics and flow",
    note: "The two areas that decide how much of real TypeScript the checker can follow.",
    steps: [
      {
        n: 8,
        title: "Generics",
        purpose:
          "Type parameters on functions, interfaces, aliases and classes: inference, substitution, constraints, defaults, explicit type arguments, recursive shapes and builtin generics.",
        link: "/architecture/generics",
        tests: ["tests/generics_tier1.rs", "tests/generics_tier2.rs", "tests/generic_classes.rs", "tests/recursive_types.rs"],
        status: "done",
      },
      {
        n: 9,
        title: "Narrowing completion, sessions and file identity",
        purpose:
          "Joins, discriminated unions, `in` and `instanceof`, and `CheckSession` for file identity and metrics.",
        link: "/architecture/narrowing",
        tests: ["tests/narrowing_advanced.rs", "tests/narrowing_complete.rs", "tests/check_session.rs"],
        status: "done",
      },
    ],
  },
  {
    id: 3,
    title: "From one file to a project",
    note: "Imports, a sturdier core, and an engine that checks a set of files and re-checks only what a change reaches.",
    steps: [
      {
        n: 10,
        title: "Module Resolution",
        purpose:
          "Resolve imports with `oxc_resolver`, build a graph indexed by `FileId`, detect cycles, order files into dependency layers and check a project in parallel. Behind the `module-resolution` feature.",
        link: "/architecture/module-resolution",
        tests: ["tests/module_resolution.rs"],
        status: "done",
      },
      {
        n: 11,
        title: "Architecture Foundation",
        purpose:
          "Salsa, a bump scratchpad that takes only `Copy` values, and mimalloc, each behind its own feature.",
        link: "/architecture/foundation-dependencies",
        status: "done",
      },
      {
        n: 12,
        title: "Declaration Slots and Named Nodes",
        purpose:
          "Interfaces, classes and object-literal aliases become `Ref` slots that are never rewritten; a name lives on a `Named` or `App` node instead of on an id.",
        link: "/architecture/arena",
        status: "done",
      },
      {
        n: 13,
        title: "Intersection Types",
        purpose:
          "`A & B` with ordered identity, lazy reduction of discriminant conflicts, relations, member access, calls and generics.",
        link: "/architecture/intersections",
        tests: ["tests/intersection_types.rs", "tests/intersection_overloads.rs"],
        status: "done",
      },
      {
        n: 14,
        title: "Cross-File Types",
        purpose:
          "A declaration gets an identity that survives a run and moves between arenas as data; imports are diagnosed against complete export lists and cycles are checked until their exports settle.",
        link: "/architecture/cross-file-types",
        tests: ["tests/unit_exports.rs", "tests/unit_imports.rs", "tests/project_imports.rs", "tests/import_diagnostics.rs"],
        status: "done",
      },
      {
        n: 15,
        title: "Project Engine",
        purpose:
          "A ready-queue scheduler, partial export lists with named reasons, cycles settled as one unit, `tsconfig.json` as a graph input, globals and library members, limits that report instead of guessing, and a query layer for incremental and watch mode.",
        link: "/architecture/project-engine",
        tests: ["tests/scheduler.rs", "tests/scc_contract.rs", "tests/project_config.rs", "tests/incremental_project.rs"],
        status: "current",
      },
    ],
  },
];
