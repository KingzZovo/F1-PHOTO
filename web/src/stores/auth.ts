import { defineStore } from "pinia"
import { computed, ref } from "vue"
import { api, silentGet, ApiError } from "@/api/client"
import type { LoginRequest, LoginResponse, LoginUser, MeResponse } from "@/api/types"

const LS_TOKEN = "f1photo.access_token"
const LS_EXPIRES = "f1photo.expires_at"
const LS_USER = "f1photo.user"

function loadFromStorage() {
  try {
    const token = localStorage.getItem(LS_TOKEN)
    const expiresAt = localStorage.getItem(LS_EXPIRES)
    const userJson = localStorage.getItem(LS_USER)
    const user = userJson ? (JSON.parse(userJson) as LoginUser) : null
    return { token, expiresAt, user }
  } catch {
    return { token: null, expiresAt: null, user: null }
  }
}

export const useAuthStore = defineStore("auth", () => {
  const initial = loadFromStorage()
  const accessToken = ref<string | null>(initial.token)
  const expiresAt = ref<string | null>(initial.expiresAt)
  const user = ref<LoginUser | null>(initial.user)
  const initializing = ref(false)
  const initialized = ref(false)

  const isAuthenticated = computed(() => !!accessToken.value && !!user.value)
  const isAdmin = computed(() => user.value?.role === "admin")
  const expiresInMs = computed(() => {
    if (!expiresAt.value) return 0
    const t = Date.parse(expiresAt.value)
    if (Number.isNaN(t)) return 0
    return Math.max(0, t - Date.now())
  })
  const isExpired = computed(() => !!accessToken.value && expiresInMs.value === 0 && !!expiresAt.value)

  function persist() {
    try {
      if (accessToken.value) localStorage.setItem(LS_TOKEN, accessToken.value)
      else localStorage.removeItem(LS_TOKEN)
      if (expiresAt.value) localStorage.setItem(LS_EXPIRES, expiresAt.value)
      else localStorage.removeItem(LS_EXPIRES)
      if (user.value) localStorage.setItem(LS_USER, JSON.stringify(user.value))
      else localStorage.removeItem(LS_USER)
    } catch {
      // ignore quota / private mode failures
    }
  }

  function setSession(payload: LoginResponse) {
    accessToken.value = payload.access_token
    expiresAt.value = payload.expires_at
    user.value = payload.user
    persist()
  }

  function clear() {
    accessToken.value = null
    expiresAt.value = null
    user.value = null
    persist()
  }

  async function login(req: LoginRequest) {
    const r = await api.post<LoginResponse>("/api/auth/login", req)
    setSession(r.data)
    return r.data
  }

  async function logout() {
    try {
      await api.post("/api/auth/logout")
    } catch {
      // server may already consider us anonymous; clear locally regardless
    }
    clear()
  }

  async function fetchMe(): Promise<LoginUser | null> {
    try {
      const me = await silentGet<MeResponse>("/api/auth/me")
      user.value = me
      persist()
      return me
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        clear()
      }
      return null
    }
  }

  // Soft-renew probe: hit /api/auth/me every minute. If the token is
  // about to expire (< 5 min) or just expired, ping the server which
  // will return 401 and trigger clear() via the response interceptor.
  let renewTimer: ReturnType<typeof setInterval> | null = null
  function startRenewTimer() {
    stopRenewTimer()
    renewTimer = setInterval(() => {
      if (!accessToken.value) return
      // No refresh endpoint exists yet; we just validate by hitting /me.
      void fetchMe()
    }, 60_000)
  }
  function stopRenewTimer() {
    if (renewTimer) {
      clearInterval(renewTimer)
      renewTimer = null
    }
  }

  async function bootstrap() {
    if (initialized.value || initializing.value) return
    initializing.value = true
    try {
      if (accessToken.value) {
        await fetchMe()
      }
    } finally {
      initializing.value = false
      initialized.value = true
      startRenewTimer()
    }
  }

  return {
    accessToken,
    expiresAt,
    user,
    initializing,
    initialized,
    isAuthenticated,
    isAdmin,
    expiresInMs,
    isExpired,
    login,
    logout,
    clear,
    fetchMe,
    bootstrap,
    startRenewTimer,
    stopRenewTimer,
  }
})
