import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";
import Journey from "./components/Journey.vue";
import Workstreams from "./components/Workstreams.vue";
import WorkOrder from "./components/WorkOrder.vue";
import Flow from "./components/Flow.vue";
import Stats from "./components/Stats.vue";
import Layers from "./components/Layers.vue";
import "./custom.css";

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component("Journey", Journey);
    app.component("Workstreams", Workstreams);
    app.component("WorkOrder", WorkOrder);
    app.component("Flow", Flow);
    app.component("Stats", Stats);
    app.component("Layers", Layers);
  },
} satisfies Theme;
