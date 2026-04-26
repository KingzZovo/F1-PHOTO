import { api } from "./client"
import type {
  Project,
  ProjectInput,
  ProjectPatch,
  Member,
  MemberAddInput,
  MemberPatch,
  MyPerms,
} from "./types"

export type ArchivedFilter = "active" | "archived" | "all"

export const projectsApi = {
  list: async (archived: ArchivedFilter = "active"): Promise<Project[]> => {
    const { data } = await api.get<Project[]>("/api/projects", {
      params: { archived },
    })
    return data
  },
  get: async (id: string): Promise<Project> => {
    const { data } = await api.get<Project>(`/api/projects/${id}`)
    return data
  },
  create: async (input: ProjectInput): Promise<Project> => {
    const { data } = await api.post<Project>("/api/projects", input)
    return data
  },
  patch: async (id: string, input: ProjectPatch): Promise<Project> => {
    const { data } = await api.patch<Project>(`/api/projects/${id}`, input)
    return data
  },
  archive: async (id: string): Promise<void> => {
    await api.delete(`/api/projects/${id}`)
  },
  unarchive: async (id: string): Promise<Project> => {
    const { data } = await api.post<Project>(`/api/projects/${id}/unarchive`)
    return data
  },
  myPerms: async (id: string): Promise<MyPerms> => {
    const { data } = await api.get<MyPerms>(`/api/projects/${id}/me`)
    return data
  },
  listMembers: async (id: string): Promise<Member[]> => {
    const { data } = await api.get<Member[]>(`/api/projects/${id}/members`)
    return data
  },
  addMember: async (id: string, input: MemberAddInput): Promise<Member> => {
    const { data } = await api.post<Member>(`/api/projects/${id}/members`, input)
    return data
  },
  patchMember: async (
    id: string,
    userId: string,
    input: MemberPatch,
  ): Promise<Member> => {
    const { data } = await api.patch<Member>(
      `/api/projects/${id}/members/${userId}`,
      input,
    )
    return data
  },
  removeMember: async (id: string, userId: string): Promise<void> => {
    await api.delete(`/api/projects/${id}/members/${userId}`)
  },
}
