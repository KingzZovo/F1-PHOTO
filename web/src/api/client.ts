import axios, { AxiosError, type AxiosInstance, type AxiosRequestConfig } from "axios"
import { useAuthStore } from "@/stores/auth"
import type { AppErrorBody } from "./types"

// Soft-renew window: if the JWT will expire within this many seconds,
// the auth store will refetch /api/auth/me to extend awareness; the
// server currently issues only access_tokens, so genuine renewal
// requires re-login. We expose a reactive "expiresIn" so the UI can
// prompt the user before forced sign-out.
export const SOFT_RENEW_WINDOW_SEC = 5 * 60

let toastFn: ((msg: string, type?: "error" | "warning" | "info" | "success") => void) | null = null

export function setApiToastSink(fn: typeof toastFn) {
  toastFn = fn
}

function showToast(msg: string, type: "error" | "warning" | "info" | "success" = "error") {
  if (toastFn) toastFn(msg, type)
  else console.warn(`[toast:${type}]`, msg)
}

export class ApiError extends Error {
  status: number
  code: string
  raw: AppErrorBody | null
  constructor(status: number, code: string, message: string, raw: AppErrorBody | null) {
    super(message)
    this.status = status
    this.code = code
    this.raw = raw
  }
}

function extractAppError(err: AxiosError): { code: string; message: string; raw: AppErrorBody | null } {
  const data = err.response?.data as AppErrorBody | undefined
  if (data && typeof data === "object" && data.error && typeof data.error === "object") {
    return { code: data.error.code || "INTERNAL", message: data.error.message || err.message, raw: data }
  }
  // Network or non-JSON failures
  if (!err.response) {
    return { code: "NETWORK", message: err.message || "Network error", raw: null }
  }
  return { code: "INTERNAL", message: err.message || "Unexpected server error", raw: null }
}

export function createApiClient(): AxiosInstance {
  const inst = axios.create({
    baseURL: "/",
    timeout: 30_000,
    headers: { "Content-Type": "application/json" },
  })

  inst.interceptors.request.use((config) => {
    const auth = useAuthStore()
    if (auth.accessToken) {
      config.headers = config.headers ?? {}
      config.headers["Authorization"] = `Bearer ${auth.accessToken}`
    }
    return config
  })

  inst.interceptors.response.use(
    (resp) => resp,
    (err: AxiosError) => {
      const auth = useAuthStore()
      const { code, message, raw } = extractAppError(err)
      const status = err.response?.status ?? 0

      // Drop credentials on auth failure so the router guard sends user back to /login.
      if (status === 401 || code === "UNAUTHORIZED") {
        auth.clear()
      }

      // Suppress noisy toast on the silent /me probe; pages can read err.code instead.
      const cfg = err.config as (AxiosRequestConfig & { _silent?: boolean }) | undefined
      if (!cfg?._silent) {
        showToast(message, status === 401 ? "warning" : "error")
      }
      return Promise.reject(new ApiError(status, code, message, raw))
    },
  )
  return inst
}

export const api = createApiClient()

export async function silentGet<T>(url: string): Promise<T> {
  const r = await api.get<T>(url, { _silent: true } as AxiosRequestConfig & { _silent: boolean })
  return r.data
}
