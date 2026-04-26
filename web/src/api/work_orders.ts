import { api } from "./client"
import type {
  PagedListResponse,
  WorkOrder,
  WorkOrderInput,
  WorkOrderListQuery,
  WorkOrderPatch,
  WorkOrderStatus,
} from "./types"

/**
 * Project-scoped work-orders client.
 *
 * Backend routes (see server/src/api/work_orders.rs):
 *   GET    /api/projects/:pid/work_orders       list (q, status, page, page_size)
 *   POST   /api/projects/:pid/work_orders       create  (admin/upload)
 *   GET    /api/projects/:pid/work_orders/:id   get one
 *   PATCH  /api/projects/:pid/work_orders/:id   patch   (admin/upload)
 *   DELETE /api/projects/:pid/work_orders/:id   hard delete (admin/delete)
 *   POST   /api/projects/:pid/work_orders/:id/transition  status state machine
 *
 * Status state machine (mirrored from server/src/api/work_orders.rs):
 *   open ->        in_progress | cancelled
 *   in_progress -> done | cancelled
 *   done ->        in_progress
 *   cancelled ->   open
 */
export function workOrdersApi(projectId: string) {
  const base = `/api/projects/${projectId}/work_orders`
  return {
    async list(query: WorkOrderListQuery = {}): Promise<PagedListResponse<WorkOrder>> {
      const params: Record<string, string | number> = {}
      if (query.q && query.q.trim().length > 0) params.q = query.q.trim()
      if (query.status) params.status = query.status
      if (typeof query.page === "number") params.page = query.page
      if (typeof query.page_size === "number") params.page_size = query.page_size
      const { data } = await api.get<PagedListResponse<WorkOrder>>(base, { params })
      return data
    },
    async get(id: string): Promise<WorkOrder> {
      const { data } = await api.get<WorkOrder>(`${base}/${id}`)
      return data
    },
    async create(input: WorkOrderInput): Promise<WorkOrder> {
      const { data } = await api.post<WorkOrder>(base, input)
      return data
    },
    async patch(id: string, patch: WorkOrderPatch): Promise<WorkOrder> {
      const { data } = await api.patch<WorkOrder>(`${base}/${id}`, patch)
      return data
    },
    async transition(id: string, status: WorkOrderStatus): Promise<WorkOrder> {
      const { data } = await api.post<WorkOrder>(`${base}/${id}/transition`, { status })
      return data
    },
    async remove(id: string): Promise<void> {
      await api.delete(`${base}/${id}`)
    },
  }
}

export type WorkOrdersApi = ReturnType<typeof workOrdersApi>

export const WO_STATUS_LABELS: Record<WorkOrderStatus, string> = {
  open: "待开工",
  in_progress: "进行中",
  done: "已完成",
  cancelled: "已取消",
}

export const WO_STATUS_TAG_TYPE: Record<
  WorkOrderStatus,
  "default" | "info" | "success" | "warning" | "error"
> = {
  open: "info",
  in_progress: "warning",
  done: "success",
  cancelled: "error",
}

/** Allowed next statuses given a current status (mirrors check_transition). */
export function allowedTransitions(from: string): WorkOrderStatus[] {
  switch (from) {
    case "open":
      return ["in_progress", "cancelled"]
    case "in_progress":
      return ["done", "cancelled"]
    case "done":
      return ["in_progress"]
    case "cancelled":
      return ["open"]
    default:
      return ["open"]
  }
}
