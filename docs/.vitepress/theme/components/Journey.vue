<script setup lang="ts">
import { computed } from "vue";
import { withBase } from "vitepress";
import { journey } from "../../data/journey";

const props = defineProps<{ part: number }>();
const part = computed(() => journey.find((p) => p.id === props.part));
</script>

<template>
  <div v-if="part" class="journey">
    <p class="journey-note">{{ part.note }}</p>
    <ol class="journey-list">
      <li v-for="step in part.steps" :key="step.n" :class="['journey-step', step.status]">
        <span class="journey-dot">{{ step.n }}</span>
        <div class="journey-card">
          <div class="journey-head">
            <a class="journey-name" :href="withBase(step.link)">{{ step.title }}</a>
            <span v-if="step.status === 'current'" class="badge current">Current baseline</span>
          </div>
          <p class="journey-purpose">{{ step.purpose }}</p>
          <p v-if="step.tests" class="journey-tests">
            <span>Guarded by</span>
            <code v-for="t in step.tests" :key="t">{{ t }}</code>
          </p>
        </div>
      </li>
    </ol>
  </div>
</template>

<style scoped>
.journey-note {
  margin: 0 0 18px;
  color: var(--vp-c-text-2);
}
.journey-list {
  list-style: none;
  margin: 0;
  padding: 0 0 0 20px;
  border-left: 2px solid var(--vp-c-divider);
}
.journey-step {
  position: relative;
  padding: 0 0 18px 28px;
}
.journey-step:last-child {
  padding-bottom: 0;
}
.journey-dot {
  position: absolute;
  left: -21px;
  top: 14px;
  width: 36px;
  height: 36px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  font-weight: 600;
  background: var(--vp-c-bg);
  border: 2px solid var(--vp-c-green-1);
  color: var(--vp-c-green-1);
}
.journey-step.current .journey-dot {
  background: var(--vp-c-brand-1);
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-white);
}
.journey-card {
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  padding: 14px 18px;
  background: var(--vp-c-bg-soft);
}
.journey-step.current .journey-card {
  border-color: var(--vp-c-brand-1);
}
.journey-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
}
.journey-name {
  font-weight: 600;
  font-size: 16px;
  text-decoration: none;
}
.journey-purpose {
  margin: 6px 0 0;
  font-size: 14px;
  line-height: 1.6;
  color: var(--vp-c-text-2);
}
.journey-tests {
  margin: 10px 0 0;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--vp-c-text-3);
}
.journey-tests code {
  font-size: 12px;
}
</style>
