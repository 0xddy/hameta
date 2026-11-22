import { defineStore } from "pinia";
import { ref } from "vue";
import {
  getMe,
  login as apiLogin,
  logout as apiLogout,
  type AuthUserInfo,
  type SessionSummary,
} from "@/api/auth";
import { getSetupStatus } from "@/api/setup";

export const useAuthStore = defineStore("auth", () => {
  const bootstrapped = ref(false);
  const initialized = ref(false);
  const version = ref("0.1.0");
  const authenticated = ref(false);
  const user = ref<AuthUserInfo | null>(null);
  const session = ref<SessionSummary | null>(null);

  async function refreshBootstrap(): Promise<void> {
    const status = await getSetupStatus();
    initialized.value = status.initialized;
    version.value = status.version;

    if (!status.initialized) {
      authenticated.value = false;
      user.value = null;
      session.value = null;
      bootstrapped.value = true;
      return;
    }

    try {
      const me = await getMe();
      user.value = me.user;
      session.value = me.session;
      authenticated.value = true;
    } catch {
      user.value = null;
      session.value = null;
      authenticated.value = false;
    } finally {
      bootstrapped.value = true;
    }
  }

  async function login(username: string, password: string): Promise<void> {
    const me = await apiLogin(username, password);
    user.value = me.user;
    session.value = me.session;
    authenticated.value = true;
    initialized.value = true;
  }

  async function logout(): Promise<void> {
    await apiLogout();
    user.value = null;
    session.value = null;
    authenticated.value = false;
  }

  return {
    bootstrapped,
    initialized,
    version,
    authenticated,
    user,
    session,
    refreshBootstrap,
    login,
    logout,
  };
});
