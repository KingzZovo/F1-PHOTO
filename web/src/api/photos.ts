import { api } from "./client"
import type { PhotoUploadInput, PhotoUploadResponse } from "./types"

/**
 * Photo upload (multipart). Server returns 202 Accepted on success and enqueues
 * the photo for recognition.
 *
 * Multipart fields (see server/src/api/photos.rs):
 *   file        (required, binary)
 *   owner_type  (required: person|tool|device|wo_raw)
 *   wo_id   | wo_code   (one of the two — links the photo to a work order)
 *   owner_id    (optional uuid)
 *   employee_no (optional, when owner_type=person)
 *   sn          (optional, when owner_type=tool|device)
 *   angle       (optional: front|side|back|unknown, default unknown)
 */
export function photosApi(projectId: string) {
  const base = `/api/projects/${projectId}/photos`
  return {
    async upload(input: PhotoUploadInput): Promise<PhotoUploadResponse> {
      const fd = new FormData()
      fd.append("file", input.file, input.file.name)
      fd.append("owner_type", input.owner_type)
      if (input.wo_id) fd.append("wo_id", input.wo_id)
      else if (input.wo_code) fd.append("wo_code", input.wo_code)
      if (input.owner_id) fd.append("owner_id", input.owner_id)
      if (input.employee_no) fd.append("employee_no", input.employee_no)
      if (input.sn) fd.append("sn", input.sn)
      if (input.angle) fd.append("angle", input.angle)

      const { data } = await api.post<PhotoUploadResponse>(base, fd, {
        headers: { "Content-Type": "multipart/form-data" },
      })
      return data
    },
  }
}

export type PhotosApi = ReturnType<typeof photosApi>
