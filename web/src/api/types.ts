export interface LoginRequest {
  username: string
  password: string
}

export interface LoginUser {
  id: string
  username: string
  full_name?: string | null
  email?: string | null
  role: "admin" | "member" | string
}

export interface LoginResponse {
  access_token: string
  token_type: string
  expires_at: string
  user: LoginUser
}

export interface Project {
  id: string
  code: string
  name: string
  icon?: string | null
  description?: string | null
  archived_at?: string | null
  created_at: string
  updated_at: string
}

export interface ProjectInput {
  code: string
  name: string
  icon?: string | null
  description?: string | null
}

export interface ProjectPatch {
  code?: string
  name?: string
  icon?: string | null
  description?: string | null
}

export interface Member {
  user_id: string
  username: string
  full_name?: string | null
  role: string
  can_view: boolean
  can_upload: boolean
  can_delete: boolean
  can_manage: boolean
  created_at: string
}

export interface MemberAddInput {
  username: string
  can_view?: boolean
  can_upload?: boolean
  can_delete?: boolean
  can_manage?: boolean
}

export interface MemberPatch {
  can_view?: boolean
  can_upload?: boolean
  can_delete?: boolean
  can_manage?: boolean
}

export interface MyPerms {
  is_admin: boolean
  archived: boolean
  can_view: boolean
  can_upload: boolean
  can_delete: boolean
  can_manage: boolean
}

export interface Page<T> {
  items: T[]
  total: number
  limit: number
  offset: number
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

export interface AppErrorBody {
  error: {
    code: string
    message: string
  }
}

export type MeResponse = LoginUser
