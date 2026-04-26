import { api } from "./client"
import type {
  Person,
  PersonInput,
  PersonPatch,
  Tool,
  ToolInput,
  ToolPatch,
  Device,
  DeviceInput,
  DevicePatch,
  PagedListResponse,
  MasterDataListQuery,
} from "./types"

// archivedFilter UI 语义 -> include_deleted 后端参数。
// active   = 只看未删除             -> include_deleted=false，前端额外过滤掉　deleted_at != null　（其实后端已过滤）
// archived = 只看已软删       -> include_deleted=true，前端过滤出 deleted_at != null
// all      = 全部                  -> include_deleted=true、不过滤
export type ArchivedFilter = "active" | "archived" | "all"

function toBackendQuery(
  filter: ArchivedFilter,
  q: MasterDataListQuery,
): Record<string, string | number | boolean | undefined> {
  return {
    q: q.q && q.q.trim() ? q.q.trim() : undefined,
    page: q.page ?? 1,
    page_size: q.page_size ?? 20,
    include_deleted: filter !== "active",
  }
}

export interface MasterDataApi<T, CreateInput, PatchInput> {
  list: (
    filter: ArchivedFilter,
    q: MasterDataListQuery,
  ) => Promise<PagedListResponse<T>>
  create: (input: CreateInput) => Promise<T>
  patch: (id: string, input: PatchInput) => Promise<T>
  softDelete: (id: string) => Promise<void>
  restore: (id: string) => Promise<T>
}

export const personsApi: MasterDataApi<Person, PersonInput, PersonPatch> = {
  list: async (filter, q) => {
    const { data } = await api.get<PagedListResponse<Person>>("/api/persons", {
      params: toBackendQuery(filter, q),
    })
    return data
  },
  create: async (input) => {
    const { data } = await api.post<Person>("/api/persons", input)
    return data
  },
  patch: async (id, input) => {
    const { data } = await api.patch<Person>(`/api/persons/${id}`, input)
    return data
  },
  softDelete: async (id) => {
    await api.delete(`/api/persons/${id}`)
  },
  restore: async (id) => {
    const { data } = await api.post<Person>(`/api/persons/${id}/restore`)
    return data
  },
}

export const toolsApi: MasterDataApi<Tool, ToolInput, ToolPatch> = {
  list: async (filter, q) => {
    const { data } = await api.get<PagedListResponse<Tool>>("/api/tools", {
      params: toBackendQuery(filter, q),
    })
    return data
  },
  create: async (input) => {
    const { data } = await api.post<Tool>("/api/tools", input)
    return data
  },
  patch: async (id, input) => {
    const { data } = await api.patch<Tool>(`/api/tools/${id}`, input)
    return data
  },
  softDelete: async (id) => {
    await api.delete(`/api/tools/${id}`)
  },
  restore: async (id) => {
    const { data } = await api.post<Tool>(`/api/tools/${id}/restore`)
    return data
  },
}

export const devicesApi: MasterDataApi<Device, DeviceInput, DevicePatch> = {
  list: async (filter, q) => {
    const { data } = await api.get<PagedListResponse<Device>>("/api/devices", {
      params: toBackendQuery(filter, q),
    })
    return data
  },
  create: async (input) => {
    const { data } = await api.post<Device>("/api/devices", input)
    return data
  },
  patch: async (id, input) => {
    const { data } = await api.patch<Device>(`/api/devices/${id}`, input)
    return data
  },
  softDelete: async (id) => {
    await api.delete(`/api/devices/${id}`)
  },
  restore: async (id) => {
    const { data } = await api.post<Device>(`/api/devices/${id}/restore`)
    return data
  },
}
