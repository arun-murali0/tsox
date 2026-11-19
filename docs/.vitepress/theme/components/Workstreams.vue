<script setup lang="ts">
import { withBase } from "vitepress";
import { workstreams, type State } from "../../data/workstreams";

const label: Record<State, string> = { done: "Done", partial: "In progress", open: "Open" };
const register = "/architecture/remaining-work-gap-register";
</script>

<template>
  <div class="ws-grid">
    <article v-for="w in workstreams" :key="w.id" :class="['ws-card', w.state]">
      <header class="ws-head">
        <span class="ws-id">{{ w.id }}</span>
        <h3 class="ws-title">{{ w.title }}</h3>
        <span :class="['badge', w.state]">{{ label[w.state] }}</span>
      </header>
      <p class="ws-why">{{ w.why }}</p>
      <p class="ws-meta">
        <span>Size: {{ w.size }}</span>
        <span>Priority: {{ w.priority }}</span>
      </p>
      <p v-if="w.done" class="ws-line"><strong>Done</strong> {{ w.done }}</p>
      <p class="ws-line"><strong>Next</strong> {{ w.next }}</p>
      <a class="ws-more" :href="withBase(register) + '#' + w.anchor">Design and checklist</a>
    </article>
  </div>
</template>

<style scoped>
.ws-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 16px;
  margin: 20px 0;
}
.ws-card {
  display: flex;
  flex-direction: column;
  border: 1px solid var(--vp-c-divider);
  border-top-width: 3px;
  border-radius: 10px;
  padding: 14px 16px;
  background: var(--vp-c-bg-soft);
}
.ws-card.done {
  border-top-color: var(--vp-c-green-1);
}
.ws-card.partial {
  border-top-color: var(--vp-c-yellow-1);
}
.ws-card.open {
  border-top-color: var(--vp-c-gray-1);
}
.ws-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.ws-id {
  font-weight: 700;
  font-size: 13px;
  color: var(--vp-c-brand-1);
}
.ws-title {
  flex: 1;
  margin: 0;
  padding: 0;
  border: 0;
  font-size: 15px;
  line-height: 1.35;
  letter-spacing: 0;
}
.ws-why {
  margin: 8px 0 0;
  font-size: 13px;
  line-height: 1.55;
  color: var(--vp-c-text-2);
}
.ws-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 14px;
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--vp-c-text-3);
}
.ws-line {
  margin: 8px 0 0;
  font-size: 13px;
  line-height: 1.55;
}
.ws-line strong {
  display: inline-block;
  min-width: 38px;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--vp-c-text-3);
}
.ws-more {
  margin-top: auto;
  padding-top: 12px;
  font-size: 13px;
  font-weight: 500;
}
</style>
