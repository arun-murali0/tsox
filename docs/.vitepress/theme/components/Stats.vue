<script setup lang="ts">
defineProps<{
  items: { value: string | number; label: string; tone?: "good" | "warn" | "bad" }[];
  caption?: string;
}>();
</script>

<template>
  <div class="stats">
    <div class="stats-row">
      <div v-for="s in items" :key="s.label" :class="['stat', s.tone]">
        <span class="stat-value">{{ s.value }}</span>
        <span class="stat-label">{{ s.label }}</span>
      </div>
    </div>
    <p v-if="caption" class="stats-caption">{{ caption }}</p>
  </div>
</template>

<style scoped>
.stats {
  margin: 20px 0;
}
.stats-row {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  gap: 12px;
}
.stat {
  display: flex;
  flex-direction: column;
  padding: 12px 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  background: var(--vp-c-bg-soft);
}
.stat-value {
  font-size: 26px;
  font-weight: 700;
  line-height: 1.2;
}
.stat.good .stat-value {
  color: var(--vp-c-green-1);
}
.stat.warn .stat-value {
  color: var(--vp-c-yellow-1);
}
.stat.bad .stat-value {
  color: var(--vp-c-red-1);
}
.stat-label {
  font-size: 13px;
  color: var(--vp-c-text-2);
}
.stats-caption {
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--vp-c-text-3);
}
</style>
