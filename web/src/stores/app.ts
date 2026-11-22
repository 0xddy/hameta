import { defineStore } from "pinia";
import { ref } from "vue";
import { fetchHealth, type HealthStatus } from "@/api/client";

export const useAppStore = defineStore("app", () => {
  const health = ref<HealthStatus | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);

  async function refreshHealth() {
    loading.value = true;
    error.value = null;
    try {
      health.value = await fetchHealth();
    } catch (err) {
      error.value = err instanceof Error ? err.message : "无法连接到 hameta 后端";
    } finally {
      loading.value = false;
    }
  }

  return { health, loading, error, refreshHealth };
});
