// API contract types matching the Rust server.

export type AppErrorBody = {
  error: { code: string; message: string }
}

export type LoginRequest = { username: string; password: string }

export type LoginUser = {
  id: string
  username: string
  role: "admin" | "member"
  full_name?: string | null
}

export type LoginResponse = {
  access_token: string
  token_type: "Bearer"
  expires_at: string // ISO-8601
  user: LoginUser
}

export type MeResponse = LoginUser

export type Project = {
  id: string
  code: string
  name: string
  description?: string | null
  archived_at?: string | null
  created_at: string
  updated_at: string
}

export type Page<T> = {
  data: T[]
  page: number
  page_size: number
  total: number
}

export type ErrorCode =
  | "UNAUTHORIZED"
  | "FORBIDDEN"
  | "NOT_FOUND"
  | "INVALID_INPUT"
  | "CONFLICT"
  | "TOO_LARGE"
  | "INTERNAL"
  | "PROJECT_FORBIDDEN"
