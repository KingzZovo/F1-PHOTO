import { api } from "./client"
import type {
  PagedListResponse,
  RecognitionCorrectInput,
  RecognitionItem,
  RecognitionListQuery,
  RecognitionStatus,
} from "./types"

/**
 * Recognition results client.
 *
 * Backend routes (see server/src/api/recognitions.rs):
 *   GET   /api/projects/:pid/recognition_items                  list (filters)
 *   GET   /api/projects/:pid/recognition_items/:id              get one
 *   PATCH /api/projects/:pid/recognition_items/:id/correct      manual correction
 *   GET   /api/projects/:pid/photos/:photo_id/detections        per-photo detections
 *   GET   /api/projects/:pid/photos/:photo_id/recognition_items per-photo items
 *
 * Reads require ViewPerm; correct requires UploadPerm.
 */
export function recognitionApi(projectId: string) {
  const base = `/api/projects/${projectId}/recognition_items`
  return {
    async list(query: RecognitionListQuery = {}): Promise<PagedListResponse<RecognitionItem>> {
      const params: Record<string, string | number> = {}
      if (query.photo_id) params.photo_id = query.photo_id
      if (query.owner_type) params.owner_type = query.owner_type
      if (query.owner_id) params.owner_id = query.owner_id
      if (query.status) params.status = query.status
      if (typeof query.page === "number") params.page = query.page
      if (typeof query.page_size === "number") params.page_size = query.page_size
      const { data } = await api.get<PagedListResponse<RecognitionItem>>(base, { params })
      return data
    },
    async get(id: string): Promise<RecognitionItem> {
      const { data } = await api.get<RecognitionItem>(`${base}/${id}`)
      return data
    },
    async correct(id: string, input: RecognitionCorrectInput): Promise<RecognitionItem> {
      const { data } = await api.patch<RecognitionItem>(`${base}/${id}/correct`, input)
      return data
    },
    async clearCorrection(id: string): Promise<RecognitionItem> {
      const { data } = await api.patch<RecognitionItem>(`${base}/${id}/correct`, {})
      return data
    },
    async listForPhoto(photoId: string): Promise<RecognitionItem[]> {
      const { data } = await api.get<RecognitionItem[]>(
        `/api/projects/${projectId}/photos/${photoId}/recognition_items`,
      )
      return data
    },
  }
}

export type RecognitionApi = ReturnType<typeof recognitionApi>

export const RECOGNITION_STATUS_LABELS: Record<RecognitionStatus, string> = {
  matched: "已匹配",
  learning: "学习中",
  unmatched: "未匹配",
  manual_corrected: "人工校正",
}

export const RECOGNITION_STATUS_TAG_TYPE: Record<
  RecognitionStatus,
  "default" | "info" | "success" | "warning" | "error"
> = {
  matched: "success",
  learning: "info",
  unmatched: "warning",
  manual_corrected: "info",
}
