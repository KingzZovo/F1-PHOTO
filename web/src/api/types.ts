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

// ---------------------------------------------------------------------------
// Master data (persons / tools / devices)
// ---------------------------------------------------------------------------

/// Backend list response shape for master data endpoints:
/// `{ data, page, page_size, total }` — distinct from the cursor-style `Page<T>`.
export interface PagedListResponse<T> {
  data: T[]
  page: number
  page_size: number
  total: number
}

export interface MasterDataListQuery {
  q?: string
  page?: number
  page_size?: number
  include_deleted?: boolean
}

export interface Person {
  id: string
  employee_no: string
  name: string
  department?: string | null
  phone?: string | null
  photo_count: number
  created_at: string
  updated_at: string
  deleted_at?: string | null
}

export interface PersonInput {
  employee_no: string
  name: string
  department?: string | null
  phone?: string | null
}

export interface PersonPatch {
  employee_no?: string
  name?: string
  department?: string | null
  phone?: string | null
}

export interface Tool {
  id: string
  sn: string
  name: string
  category?: string | null
  photo_count: number
  created_at: string
  updated_at: string
  deleted_at?: string | null
}

export interface ToolInput {
  sn: string
  name: string
  category?: string | null
}

export interface ToolPatch {
  sn?: string
  name?: string
  category?: string | null
}

export interface Device {
  id: string
  sn: string
  name: string
  model?: string | null
  photo_count: number
  created_at: string
  updated_at: string
  deleted_at?: string | null
}

export interface DeviceInput {
  sn: string
  name: string
  model?: string | null
}

export interface DevicePatch {
  sn?: string
  name?: string
  model?: string | null
}

// ---------------------------------------------------------------------------
// Work orders
// ---------------------------------------------------------------------------

export type WorkOrderStatus = "open" | "in_progress" | "done" | "cancelled"

export interface WorkOrder {
  id: string
  project_id: string
  code: string
  title?: string | null
  status: WorkOrderStatus | string
  created_by?: string | null
  created_at: string
  updated_at: string
}

export interface WorkOrderInput {
  code: string
  title?: string | null
  status?: WorkOrderStatus | null
}

export interface WorkOrderPatch {
  code?: string
  title?: string | null
  status?: WorkOrderStatus
}

export interface WorkOrderListQuery {
  q?: string
  status?: WorkOrderStatus | ""
  page?: number
  page_size?: number
}

// ---------------------------------------------------------------------------
// Photo uploads (multipart) + recognition items
// ---------------------------------------------------------------------------

export type OwnerType = "person" | "tool" | "device" | "wo_raw"
export type AngleKind = "front" | "side" | "back" | "unknown"

export interface PhotoUploadInput {
  /** Local file payload to upload. */
  file: File
  /** Owner classification of the depicted subject. */
  owner_type: OwnerType
  /** Either the work_order UUID, or its `code` within the project. */
  wo_id?: string
  wo_code?: string
  /** Optional pre-resolved owner UUID. */
  owner_id?: string
  /** Hints for owner resolution when owner_id is unset. */
  employee_no?: string
  sn?: string
  /** Optional capture angle (defaults to "unknown"). */
  angle?: AngleKind
}

export interface PhotoUploadResponse {
  id: string
  hash: string
  status: string
  deduped: boolean
  work_order_id?: string | null
  owner_type?: string | null
  owner_id?: string | null
  angle: string
}
