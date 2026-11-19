<script setup lang="ts">
// A row of boxes joined by arrows. `steps` is a list of titles, or of { title, note } objects.
// It wraps on narrow screens, and the arrows turn to point down when it does.
interface Item {
  title: string;
  note?: string;
}
defineProps<{ steps: (string | Item)[] }>();

const norm = (s: string | Item): Item => (typeof s === "string" ? { title: s } : s);
</script>

<template>
  <ol class="flow">
    <li v-for="(s, i) in steps" :key="i" class="flow-item">
      <div class="flow-box">
        <span class="flow-title">{{ norm(s).title }}</span>
        <span v-if="norm(s).note" class="flow-note">{{ norm(s).note }}</span>
      </div>
      <span v-if="i < steps.length - 1" class="flow-arrow" aria-hidden="true">&rarr;</span>
    </li>
  </ol>
</template>

<style scoped>
.flow {
  list-style: none;
  display: flex;
  flex-wrap: wrap;
  align-items: stretch;
  gap: 8px;
  margin: 20px 0;
  padding: 0;
}
.flow-item {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
}
.flow-box {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px 14px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  background: var(--vp-c-bg-soft);
}
.flow-title {
  font-size: 14px;
  font-weight: 600;
}
.flow-note {
  font-size: 12px;
  line-height: 1.4;
  color: var(--vp-c-text-3);
  max-width: 190px;
}
.flow-arrow {
  color: var(--vp-c-text-3);
  font-size: 18px;
}
</style>
