<template>
  <div class="p-6 space-y-4">
    <div class="flex items-center justify-between flex-wrap gap-2">
      <h1 class="text-2xl font-semibold">识别结果与校正</h1>
      <n-space size="small">
        <n-input
          v-model:value="photoIdInput"
          placeholder="按照片 ID 过滤（可选）"
          clearable
          style="width: 280px"
          @keyup.enter="reload"
          @clear="reload"
        />
        <n-select
          v-model:value="statusFilter"
          :options="statusOptions"
          placeholder="全部状态"
          clearable
          style="width: 150px"
          @update:value="reload"
        />
        <n-select
          v-model:value="ownerTypeFilter"
          :options="ownerTypeFilterOptions"
          placeholder="全部类型"
          clearable
          style="width: 140px"
          @update:value="reload"
        />
        <n-button @click="reload">刷新</n-button>
      </n-space>
    </div>

    <n-data-table
      :columns="columns"
      :data="rows"
      :loading="loading"
      :row-key="(r: RecognitionItem) => r.id"
      :bordered="false"
    />

    <div class="flex justify-end">
      <n-pagination
        v-model:page="page"
        v-model:page-size="pageSize"
        :item-count="total"
        :page-sizes="[10, 20, 50, 100]"
        show-size-picker
        @update:page="reload"
        @update:page-size="onPageSizeChange"
      />
    </div>

    <!-- Correct modal -->
    <n-modal v-model:show="showCorrect" preset="card" title="人工校正" style="max-width: 520px" :mask-closable="false">
      <n-form label-placement="left" label-width="80">
        <n-form-item label="当前状态">
          <span v-text="editingTarget ? statusLabel(editingTarget.status) : '-'" />
        </n-form-item>
        <n-form-item label="识别建议">
          <span v-text="editingSuggestion" />
        </n-form-item>
        <n-form-item label="所属类型">
          <n-radio-group v-model:value="correctType" @update:value="onCorrectTypeChange">
            <n-radio-button value="person">人员</n-radio-button>
            <n-radio-button value="tool">工具</n-radio-button>
            <n-radio-button value="device">设备</n-radio-button>
          </n-radio-group>
        </n-form-item>
        <n-form-item label="所属对象">
          <n-select
            v-model:value="correctOwnerId"
            :options="ownerOptions"
            :loading="ownerLoading"
            placeholder="输入关键字搜索"
            filterable
            remote
            clearable
            :clear-filter-after-select="false"
            @search="onOwnerSearch"
          />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="space-between">
          <n-button :disabled="!editingTarget?.corrected_owner_id" @click="clearCorrection">
            清除校正
          </n-button>
          <n-space>
            <n-button @click="showCorrect = false">取消</n-button>
            <n-button type="primary" :loading="submitting" :disabled="!correctOwnerId" @click="submitCorrect">
              提交校正
            </n-button>
          </n-space>
        </n-space>
      </template>
    </n-modal>
  </div>
</template>

<script setup lang="ts">
import { computed, h, onMounted, ref, watch } from "vue"
import {
  NButton,
  NDataTable,
  NForm,
  NFormItem,
  NInput,
  NModal,
  NPagination,
  NRadioButton,
  NRadioGroup,
  NSelect,
  NSpace,
  NTag,
  useMessage,
  type DataTableColumns,
  type SelectOption,
} from "naive-ui"
import { useAuthStore } from "@/stores/auth"
import {
  RECOGNITION_STATUS_LABELS,
  RECOGNITION_STATUS_TAG_TYPE,
  recognitionApi,
} from "@/api/recognition"
import { devicesApi, personsApi, toolsApi } from "@/api/master"
import type {
  CorrectionOwnerType,
  OwnerType,
  RecognitionItem,
  RecognitionStatus,
} from "@/api/types"

const DEFAULT_PROJECT_ID = "00000000-0000-0000-0000-000000000001"
const projectId = DEFAULT_PROJECT_ID

const auth = useAuthStore()
const message = useMessage()
const api = recognitionApi(projectId)

const canCorrect = computed(() => auth.isAdmin)

const statusOptions: SelectOption[] = [
  { label: "已匹配", value: "matched" },
  { label: "学习中", value: "learning" },
  { label: "未匹配", value: "unmatched" },
  { label: "人工校正", value: "manual_corrected" },
]

const ownerTypeFilterOptions: SelectOption[] = [
  { label: "人员", value: "person" },
  { label: "工具", value: "tool" },
  { label: "设备", value: "device" },
  { label: "现场原图", value: "wo_raw" },
]

const rows = ref<RecognitionItem[]>([])
const total = ref(0)
const loading = ref(false)
const page = ref(1)
const pageSize = ref(20)
const photoIdInput = ref("")
const statusFilter = ref<RecognitionStatus | null>(null)
const ownerTypeFilter = ref<OwnerType | null>(null)

function statusLabel(status: string): string {
  return (RECOGNITION_STATUS_LABELS as Record<string, string>)[status] || status
}

async function reload() {
  loading.value = true
  try {
    const photoId = photoIdInput.value.trim() || undefined
    const res = await api.list({
      photo_id: photoId,
      status: statusFilter.value || undefined,
      owner_type: ownerTypeFilter.value || undefined,
      page: page.value,
      page_size: pageSize.value,
    })
    rows.value = res.data
    total.value = res.total
  } finally {
    loading.value = false
  }
}

function onPageSizeChange(size: number) {
  pageSize.value = size
  page.value = 1
  reload()
}

onMounted(reload)

// --- Correction modal state ---
const showCorrect = ref(false)
const submitting = ref(false)
const editingTarget = ref<RecognitionItem | null>(null)
const correctType = ref<CorrectionOwnerType>("person")
const correctOwnerId = ref<string | null>(null)
const ownerOptions = ref<SelectOption[]>([])
const ownerLoading = ref(false)

const editingSuggestion = computed(() => {
  const t = editingTarget.value
  if (!t) return "-"
  const ownerType = t.suggested_owner_type || "-"
  const ownerId = t.suggested_owner_id || "-"
  const score = typeof t.suggested_score === "number" ? t.suggested_score.toFixed(3) : "-"
  return `${ownerType} · ${ownerId} · score=${score}`
})

async function searchOwners(keyword: string) {
  ownerLoading.value = true
  try {
    const q = { q: keyword, page: 1, page_size: 20 }
    if (correctType.value === "person") {
      const res = await personsApi.list("active", q)
      ownerOptions.value = res.data.map((p) => ({
        label: `${p.employee_no} · ${p.name}`,
        value: p.id,
      }))
    } else if (correctType.value === "tool") {
      const res = await toolsApi.list("active", q)
      ownerOptions.value = res.data.map((t) => ({
        label: `${t.sn} · ${t.name}`,
        value: t.id,
      }))
    } else {
      const res = await devicesApi.list("active", q)
      ownerOptions.value = res.data.map((d) => ({
        label: `${d.sn} · ${d.name}`,
        value: d.id,
      }))
    }
  } finally {
    ownerLoading.value = false
  }
}

function onOwnerSearch(keyword: string) {
  searchOwners(keyword)
}

function onCorrectTypeChange() {
  correctOwnerId.value = null
  ownerOptions.value = []
  searchOwners("")
}

watch(showCorrect, (v) => {
  if (v) {
    ownerOptions.value = []
    searchOwners("")
  }
})

function openCorrect(item: RecognitionItem) {
  editingTarget.value = item
  // Pre-fill from existing correction or suggestion if of a correctable type.
  const seedType =
    (item.corrected_owner_type as CorrectionOwnerType | undefined) ||
    (item.suggested_owner_type === "person" ||
    item.suggested_owner_type === "tool" ||
    item.suggested_owner_type === "device"
      ? (item.suggested_owner_type as CorrectionOwnerType)
      : "person")
  correctType.value = seedType
  correctOwnerId.value = item.corrected_owner_id || null
  showCorrect.value = true
}

async function submitCorrect() {
  if (!editingTarget.value || !correctOwnerId.value) return
  submitting.value = true
  try {
    await api.correct(editingTarget.value.id, {
      owner_type: correctType.value,
      owner_id: correctOwnerId.value,
    })
    message.success("已提交校正")
    showCorrect.value = false
    reload()
  } finally {
    submitting.value = false
  }
}

async function clearCorrection() {
  if (!editingTarget.value) return
  submitting.value = true
  try {
    await api.clearCorrection(editingTarget.value.id)
    message.success("已清除校正")
    showCorrect.value = false
    reload()
  } finally {
    submitting.value = false
  }
}

// --- Columns ---
const columns: DataTableColumns<RecognitionItem> = [
  {
    title: "创建时间",
    key: "created_at",
    width: 170,
    render: (r) => new Date(r.created_at).toLocaleString(),
  },
  {
    title: "状态",
    key: "status",
    width: 110,
    render: (r) => {
      const s = r.status as RecognitionStatus
      const label =
        (RECOGNITION_STATUS_LABELS as Record<string, string>)[s] || r.status
      const type =
        (RECOGNITION_STATUS_TAG_TYPE as Record<string, "default" | "info" | "success" | "warning" | "error">)[s] || "default"
      return h(NTag, { type, size: "small" }, () => label)
    },
  },
  {
    title: "识别建议",
    key: "suggested",
    render: (r) => {
      if (!r.suggested_owner_type) return "-"
      const score =
        typeof r.suggested_score === "number" ? r.suggested_score.toFixed(3) : "-"
      return `${r.suggested_owner_type} · ${r.suggested_owner_id || "-"} · ${score}`
    },
  },
  {
    title: "最终所属",
    key: "effective",
    render: (r) => {
      if (!r.effective_owner_type) return "-"
      return `${r.effective_owner_type} · ${r.effective_owner_id || "-"}`
    },
  },
  {
    title: "照片",
    key: "photo_id",
    width: 280,
    render: (r) =>
      h(
        "span",
        {
          class: "font-mono text-xs cursor-pointer text-blue-600",
          onClick: () => {
            photoIdInput.value = r.photo_id
            page.value = 1
            reload()
          },
          title: "点击过滤",
        },
        r.photo_id,
      ),
  },
  {
    title: "操作",
    key: "actions",
    width: 140,
    render: (r) => {
      if (!canCorrect.value) return h("span", "-")
      return h(
        NButton,
        { size: "tiny", type: "primary", onClick: () => openCorrect(r) },
        () => (r.corrected_owner_id ? "重新校正" : "人工校正"),
      )
    },
  },
]
</script>
