import { defineConfig } from "vitepress";

// The site is the repository's docs/ folder. Set DOCS_BASE (for example "/tsox/") when it is
// served from a sub-path, as on GitHub Pages.
export default defineConfig({
  title: "tsox",
  description: "A TypeScript-semantics compiler and type checker written in Rust.",
  base: process.env.DOCS_BASE ?? "/",
  cleanUrls: true,
  lastUpdated: true,
  // conformance/ holds one page (the part 01-05 baseline) and its raw CSV, which is not a page.
  srcExclude: ["architecture/current-workflow-notes.md", "node_modules/**"],
  // A broken link between documents fails the build. Links into the source tree are written as
  // full GitHub URLs, so none is exempt.
  ignoreDeadLinks: false,
  themeConfig: {
    nav: [
      { text: "Guide", link: "/guide/install" },
      { text: "How it was built", link: "/journey" },
      { text: "Architecture", link: "/architecture" },
      { text: "Work plan", link: "/plan" },
      { text: "Contributing", link: "/contributing" },
      { text: "GitHub", link: "https://github.com/tsoxide/tsox" },
    ],
    sidebar: [
      {
        text: "Start here",
        items: [
          { text: "How tsox was built", link: "/journey" },
          { text: "Work plan (W1 to W10)", link: "/plan" },
          { text: "Roadmap", link: "/roadmap" },
          { text: "Language coverage", link: "/language-coverage" },
          { text: "Conformance baseline", link: "/conformance/baseline-part01-05" },
          { text: "Documentation index", link: "/README" },
        ],
      },
      {
        text: "Guide",
        items: [
          { text: "Install", link: "/guide/install" },
          { text: "Command line", link: "/guide/cli" },
          { text: "Editors", link: "/guide/editors" },
          { text: "npm (WebAssembly)", link: "/guide/npm" },
          { text: "Try a build", link: "/guide/try-it" },
          { text: "Reporting problems", link: "/guide/feedback" },
        ],
      },
      {
        text: "Why it is built this way",
        collapsed: true,
        items: [
          { text: "Case study", link: "/case-study" },
          { text: "Decisions", link: "/decisions" },
          { text: "Purpose and design boundaries", link: "/purpose-and-overdesign" },
          { text: "Architecture", link: "/architecture" },
        ],
      },
      {
        text: "One file at a time",
        collapsed: true,
        items: [
          { text: "Foundation", link: "/foundation" },
          { text: "Structural types", link: "/structural-types" },
          { text: "Literal widening", link: "/literal-widening" },
          { text: "Control-flow narrowing", link: "/architecture/narrowing" },
          { text: "Classes and inheritance", link: "/classes-inheritance" },
          { text: "Expression and program checking", link: "/expression-program-checking" },
          { text: "Parameters and destructuring", link: "/parameters-destructuring" },
        ],
      },
      {
        text: "The type system",
        collapsed: true,
        items: [
          { text: "Arena and declaration slots", link: "/architecture/arena" },
          { text: "Generics", link: "/architecture/generics" },
          { text: "Intersections", link: "/architecture/intersections" },
          { text: "Flow", link: "/architecture/flow" },
          { text: "Registries", link: "/architecture/registries" },
          { text: "Diagnostic owners", link: "/architecture/diagnostic-owners" },
        ],
      },
      {
        text: "The project engine",
        collapsed: true,
        items: [
          { text: "Overview", link: "/architecture/project-engine" },
          { text: "Module resolution", link: "/architecture/module-resolution" },
          { text: "Cross-file types", link: "/architecture/cross-file-types" },
          { text: "Cycles and settlement", link: "/architecture/cycles-and-settlement" },
          { text: "Partial results", link: "/architecture/partial-results" },
          { text: "Scheduler", link: "/architecture/scheduler" },
          { text: "Project configuration", link: "/architecture/project-configuration" },
          { text: "Library and ambient declarations", link: "/architecture/library-and-ambient-declarations" },
          { text: "Query layer", link: "/architecture/query-layer" },
          { text: "Safety net and limits", link: "/architecture/safety-net-and-limits" },
          { text: "Foundation dependencies", link: "/architecture/foundation-dependencies" },
        ],
      },
      {
        text: "Tooling and layout",
        collapsed: true,
        items: [
          { text: "Workspace and crates", link: "/architecture/workspace" },
          { text: "Language server", link: "/architecture/lsp" },
        ],
      },
      {
        text: "Plans and progress",
        collapsed: true,
        items: [
          { text: "Modular architecture proposal", link: "/architecture/modular-architecture-proposal" },
          { text: "Progress on the new architecture", link: "/architecture/new-architecture-progress" },
          { text: "Remaining work and gap register", link: "/architecture/remaining-work-gap-register" },
        ],
      },
      {
        text: "Contributing",
        collapsed: true,
        items: [
          { text: "Contributing", link: "/contributing" },
          { text: "The corpus map", link: "/corpus" },
          { text: "Project structure", link: "/project-structure" },
          { text: "Testing and performance", link: "/testing" },
          { text: "Compatibility harness", link: "/compatibility-harness" },
          { text: "Maintaining the repository", link: "/maintainers" },
        ],
      },
    ],
    search: { provider: "local" },
    socialLinks: [{ icon: "github", link: "https://github.com/tsoxide/tsox" }],
    editLink: {
      pattern: "https://github.com/tsoxide/tsox/edit/main/docs/:path",
    },
    footer: { message: "MIT OR Apache-2.0" },
  },
});
