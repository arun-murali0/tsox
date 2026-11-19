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
  // The design notes link to source files and to each other by repository path.
  ignoreDeadLinks: false,
  themeConfig: {
    nav: [
      { text: "Guide", link: "/guide/install" },
      { text: "Architecture", link: "/architecture" },
      { text: "Contributing", link: "/contributing" },
      { text: "Roadmap", link: "/roadmap" },
      { text: "GitHub", link: "https://github.com/tsoxide/tsox" },
    ],
    sidebar: [
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
        text: "Contributing",
        items: [
          { text: "Contributing", link: "/contributing" },
          { text: "The corpus map", link: "/corpus" },
          { text: "Project structure", link: "/project-structure" },
          { text: "Compatibility harness", link: "/compatibility-harness" },
          { text: "Maintaining the repository", link: "/maintainers" },
        ],
      },
      {
        text: "Project",
        items: [
          { text: "Overview", link: "/README" },
          { text: "Purpose", link: "/purpose-and-overdesign" },
          { text: "Roadmap", link: "/roadmap" },
          { text: "Language coverage", link: "/language-coverage" },
          { text: "Decisions", link: "/decisions" },
          { text: "Testing", link: "/testing" },
        ],
      },
      {
        text: "How it works",
        items: [
          { text: "Architecture", link: "/architecture" },
          { text: "Foundation", link: "/foundation" },
          { text: "Structural types", link: "/structural-types" },
          { text: "Classes and inheritance", link: "/classes-inheritance" },
          { text: "Literal widening", link: "/literal-widening" },
          { text: "Parameters and destructuring", link: "/parameters-destructuring" },
          { text: "Expression and program checking", link: "/expression-program-checking" },
        ],
      },
      {
        text: "Design notes",
        collapsed: true,
        items: [
          { text: "Progress", link: "/architecture/new-architecture-progress" },
          { text: "Modular architecture proposal", link: "/architecture/modular-architecture-proposal" },
          { text: "Registries", link: "/architecture/registries" },
          { text: "Generics", link: "/architecture/generics" },
          { text: "Narrowing", link: "/architecture/narrowing" },
          { text: "Flow", link: "/architecture/flow" },
          { text: "Module resolution", link: "/architecture/module-resolution" },
          { text: "Project engine", link: "/architecture/project-engine" },
          { text: "Query layer", link: "/architecture/query-layer" },
          { text: "Language server", link: "/architecture/lsp" },
          { text: "Diagnostic owners", link: "/architecture/diagnostic-owners" },
          { text: "Remaining work", link: "/architecture/remaining-work-gap-register" },
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
