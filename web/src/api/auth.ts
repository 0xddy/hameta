import { apiRequest } from "./client";

export interface AuthUserInfo {
  id: number;
  username: string;
  last_login_at: number | null;
  created_at: number;
}

export interface SessionSummary {
  client_ip: string;
  user_agent: string;
  created_at: number;
  expires_at: number;
}

export interface AuthMeResponse {
  user: AuthUserInfo;
  session: SessionSummary;
}

export function login(username: string, password: string): Promise<AuthMeResponse> {
  return apiRequest("/api/v1/auth/login", {
    method: "POST",
    body: JSON.stringify({ username, password }),
  });
}

export function logout(): Promise<{ ok: boolean }> {
  return apiRequest("/api/v1/auth/logout", {
    method: "POST",
  });
}

export function getMe(): Promise<AuthMeResponse> {
  return apiRequest("/api/v1/auth/me");
}

export function changePassword(
  current_password: string,
  new_password: string,
): Promise<{ ok: boolean; message: string }> {
  return apiRequest("/api/v1/auth/password", {
    method: "POST",
    body: JSON.stringify({ current_password, new_password }),
  });
}
