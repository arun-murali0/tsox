<script setup lang="ts">
// The crate layers, top first. A crate may depend only on crates in the rows below it. The
// rows mirror LAYERS in crates/tsox-xtask/src/bin/xtask/layering.rs, which CI enforces.
const layers: { crates: string[]; owns: string }[] = [
  { crates: ["tsox-cli", "tsox-lsp", "tsox-wasm", "tsox-emit", "tsox-xtask"], owns: "Front ends, JavaScript emit and the development tools. None depends on another." },
  { crates: ["tsox-ir"], owns: "A typed mid-level IR skeleton. Nothing lowers into it yet." },
  { crates: ["tsox"], owns: "The facade: the public API, re-exported from the layers below." },
  { crates: ["tsox-db"], owns: "The Salsa query layer: what has to run again after a change." },
  { crates: ["tsox-project"], owns: "Many files: module graph, scheduler, cycles, project configuration." },
  { crates: ["tsox-checker"], owns: "One file as a unit of work: the bridge, passes and the unit output." },
  { crates: ["tsox-model"], owns: "The semantic model tables that emit and the language server read." },
  { crates: ["tsox-types"], owns: "The type system: arena, subtyping, generics, type display." },
  { crates: ["tsox-base"], owns: "Identities, hashing, diagnostics and codes, check options, limits." },
];
</script>

<template>
  <ol class="layers">
    <li v-for="(l, i) in layers" :key="i" class="layer">
      <span class="layer-crates">
        <code v-for="c in l.crates" :key="c">{{ c }}</code>
      </span>
      <span class="layer-owns">{{ l.owns }}</span>
    </li>
  </ol>
  <p class="layers-note">A crate depends only on crates below it. <code>cargo xtask layering</code> fails the build otherwise.</p>
</template>

<style scoped>
.layers {
  list-style: none;
  margin: 20px 0 8px;
  padding: 0;
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  overflow: hidden;
}
.layer {
  display: grid;
  grid-template-columns: minmax(180px, 38%) 1fr;
  gap: 14px;
  align-items: center;
  margin: 0;
  padding: 10px 16px;
  border-top: 1px solid var(--vp-c-divider);
}
.layer:first-child {
  border-top: 0;
}
.layer:nth-child(odd) {
  background: var(--vp-c-bg-soft);
}
.layer-crates {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.layer-crates code {
  font-size: 12px;
}
.layer-owns {
  font-size: 13px;
  line-height: 1.5;
  color: var(--vp-c-text-2);
}
.layers-note {
  margin: 0 0 20px;
  font-size: 12px;
  color: var(--vp-c-text-3);
}
@media (max-width: 640px) {
  .layer {
    grid-template-columns: 1fr;
    gap: 6px;
  }
}
</style>
