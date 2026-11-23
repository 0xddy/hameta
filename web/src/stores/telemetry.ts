import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { getFilterState } from "@/api/filter";
import { getOverviewSnapshot, type OverviewSnapshot } from "@/api/overview";

export type OverviewSseStatus = "connecting" | "connected" | "reconnecting" | "disconnected";
export type ConsoleBadgeMode = "SSE" | "PAUSE" | "OFF";

export const useTelemetryStore = defineStore("telemetry", () => {
  const snapshot = ref<OverviewSnapshot | null>(null);
  const sseStatus = ref<OverviewSseStatus>("disconnected");
  const loading = ref(false);
  const filterHasDraft = ref(false);
  const consoleBadge = ref<ConsoleBadgeMode>("SSE");

  let eventSource: EventSource | null = null;
  let reconnectTimer: number | null = null;
  let reconnectAttempt = 0;
  let activeConsumers = 0;

  const hasCriticalAlert = computed(() => {
    const h = snapshot.value?.health;
    if (!h) return false;
    return !h.redis_ok || !h.manticore_ok || h.crashed_workers > 0;
  });

  const runningWorkerCount = computed(() => {
    const h = snapshot.value?.health;
    if (!h) return 0;
    return h.running_crawlers + h.running_indexers;
  });

  const totalWorkerCount = computed(() => {
    const h = snapshot.value?.health;
    if (!h) return 0;
    return h.total_crawlers + h.total_indexers;
  });

  async function refreshSnapshot(): Promise<void> {
    loading.value = true;
    try {
      snapshot.value = await getOverviewSnapshot();
    } finally {
      loading.value = false;
    }
  }

  async function refreshFilterDraftBadge(): Promise<void> {
    try {
      const st = await getFilterState();
      filterHasDraft.value = Boolean(st.has_draft);
    } catch {
      // ignore non-critical badge probe error
    }
  }

  function openStream() {
    if (eventSource) {
      eventSource.close();
      eventSource = null;
    }
    sseStatus.value = reconnectAttempt > 0 ? "reconnecting" : "connecting";

    const es = new EventSource("/api/v1/overview/stream", { withCredentials: true });
    eventSource = es;

    es.addEventListener("snapshot", (ev: MessageEvent<string>) => {
      try {
        snapshot.value = JSON.parse(ev.data) as OverviewSnapshot;
        sseStatus.value = "connected";
        reconnectAttempt = 0;
      } catch {
        // ignore malformed frame
      }
    });

    es.onerror = () => {
      sseStatus.value = "reconnecting";
      es.close();
      if (eventSource === es) eventSource = null;
      if (activeConsumers <= 0) {
        sseStatus.value = "disconnected";
        return;
      }
      const delayMs = Math.min(15_000, 1500 * Math.pow(2, Math.min(reconnectAttempt, 4)));
      reconnectAttempt += 1;
      if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
      reconnectTimer = window.setTimeout(() => {
        if (activeConsumers > 0) openStream();
      }, delayMs);
    };
  }

  function connect() {
    activeConsumers += 1;
    if (!snapshot.value) {
      void refreshSnapshot().catch(() => {});
    }
    void refreshFilterDraftBadge();
    if (!eventSource) {
      openStream();
    }
  }

  function disconnect(force = false) {
    activeConsumers = force ? 0 : Math.max(0, activeConsumers - 1);
    if (activeConsumers === 0 || force) {
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      if (eventSource) {
        eventSource.close();
        eventSource = null;
      }
      sseStatus.value = "disconnected";
      reconnectAttempt = 0;
    }
  }

  return {
    snapshot,
    sseStatus,
    loading,
    filterHasDraft,
    consoleBadge,
    hasCriticalAlert,
    runningWorkerCount,
    totalWorkerCount,
    refreshSnapshot,
    refreshFilterDraftBadge,
    connect,
    disconnect,
  };
});
