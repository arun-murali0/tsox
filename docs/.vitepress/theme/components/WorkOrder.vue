<script setup lang="ts">
import { order } from "../../data/workstreams";
</script>

<template>
  <ol class="order">
    <li v-for="r in order" :key="r.n" class="order-row">
      <span class="order-n">Phase {{ r.n }}</span>
      <span class="order-work">
        <span v-for="w in r.work" :key="w" class="order-chip">{{ w }}</span>
      </span>
      <span class="order-why">{{ r.why }}</span>
    </li>
  </ol>
</template>

<style scoped>
.order {
  list-style: none;
  margin: 20px 0;
  padding: 0;
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  overflow: hidden;
}
.order-row {
  display: grid;
  grid-template-columns: 76px 110px 1fr;
  gap: 12px;
  align-items: start;
  padding: 12px 16px;
  margin: 0;
  border-top: 1px solid var(--vp-c-divider);
}
.order-row:first-child {
  border-top: 0;
}
.order-row:nth-child(odd) {
  background: var(--vp-c-bg-soft);
}
.order-n {
  font-size: 13px;
  font-weight: 600;
  color: var(--vp-c-text-2);
  padding-top: 2px;
}
.order-work {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.order-chip {
  font-size: 12px;
  font-weight: 700;
  padding: 2px 8px;
  border-radius: 999px;
  color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-soft);
}
.order-why {
  font-size: 14px;
  line-height: 1.55;
  color: var(--vp-c-text-2);
}
@media (max-width: 640px) {
  .order-row {
    grid-template-columns: 1fr;
    gap: 6px;
  }
}
</style>
