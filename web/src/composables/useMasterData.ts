import { computed, onMounted, reactive, ref } from "vue"
import type { MasterDataApi, ArchivedFilter } from "@/api/master"
import type { MasterDataListQuery, PagedListResponse } from "@/api/types"

export interface UseMasterDataOptions {
  defaultPageSize?: number
}

export interface MasterRowLike {
  id: string
  deleted_at?: string | null
}

/**
 * Shared state container for master-data pages (persons / tools / devices).
 *
 * Backend only supports `include_deleted=bool`, so to render the three
 * archived states we ask the backend for `include_deleted=(filter !== "active")`
 * and then narrow client-side for the "archived only" view.
 */
export function useMasterData<T extends MasterRowLike, CreateInput, PatchInput>(
  apiClient: MasterDataApi<T, CreateInput, PatchInput>,
  options: UseMasterDataOptions = {},
) {
  const defaultPageSize = options.defaultPageSize ?? 20

  const rows = ref<T[]>([]) as { value: T[] }
  const total = ref(0)
  const loading = ref(false)

  const query = reactive<
    Required<Pick<MasterDataListQuery, "q" | "page" | "page_size">>
  >({
    q: "",
    page: 1,
    page_size: defaultPageSize,
  })
  const archivedFilter = ref<ArchivedFilter>("active")

  const visibleRows = computed<T[]>(() => {
    if (archivedFilter.value === "archived") {
      return rows.value.filter((r) => r.deleted_at)
    }
    return rows.value
  })

  const visibleTotal = computed(() => {
    // total 是后端拼接后的统计；“只看归档”时后端还会返回未删除的行，
    // 使用本页 visibleRows.length 作为近似总数（分页导航仍按后端 total）。
    if (archivedFilter.value === "archived") return visibleRows.value.length
    return total.value
  })

  async function reload() {
    loading.value = true
    try {
      const resp: PagedListResponse<T> = await apiClient.list(
        archivedFilter.value,
        {
          q: query.q,
          page: query.page,
          page_size: query.page_size,
        },
      )
      rows.value = resp.data
      total.value = resp.total
    } catch (_e) {
      // axios interceptor toasts already
    } finally {
      loading.value = false
    }
  }

  function onSearch() {
    query.page = 1
    return reload()
  }

  function onArchivedChange(_v: ArchivedFilter) {
    query.page = 1
    return reload()
  }

  function onPageChange(p: number) {
    query.page = p
    return reload()
  }

  function onPageSizeChange(ps: number) {
    query.page = 1
    query.page_size = ps
    return reload()
  }

  async function create(input: CreateInput) {
    const created = await apiClient.create(input)
    await reload()
    return created
  }

  async function patch(id: string, input: PatchInput) {
    const updated = await apiClient.patch(id, input)
    await reload()
    return updated
  }

  async function softDelete(id: string) {
    await apiClient.softDelete(id)
    await reload()
  }

  async function restore(id: string) {
    await apiClient.restore(id)
    await reload()
  }

  onMounted(reload)

  return {
    rows,
    visibleRows,
    total,
    visibleTotal,
    loading,
    query,
    archivedFilter,
    reload,
    onSearch,
    onArchivedChange,
    onPageChange,
    onPageSizeChange,
    create,
    patch,
    softDelete,
    restore,
  }
}
