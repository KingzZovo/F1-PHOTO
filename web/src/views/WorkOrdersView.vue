<template>
  <div class="p-6 space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-2xl font-semibold">工单</h1>
      <n-space>
        <n-input
          v-model:value="q"
          placeholder="搜索 工单号/标题"
          clearable
          style="width: 240px"
          @keyup.enter="reload"
          @clear="reload"
        />
        <n-select
          v-model:value="statusFilter"
          :options="statusOptions"
          placeholder="全部状态"
          clearable
          style="width: 140px"
          @update:value="reload"
        />
        <n-button @click="reload">搜索</n-button>
        <n-button v-if="canManage" type="primary" @click="openCreate">+ 新建工单</n-button>
      </n-space>
    </div>

    <n-data-table
      :columns="columns"
      :data="rows"
      :loading="loading"
      :row-key="(r: WorkOrder) => r.id"
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

    <!-- Create / edit modal -->
    <n-modal v-model:show="showForm" preset="card" :title="editingId ? '编辑工单' : '新建工单'" style="max-width: 480px" :mask-closable="false">
      <n-form ref="formRef" :model="form" :rules="rules" label-placement="left" label-width="80" require-mark-placement="right-hanging">
        <n-form-item label="工单号" path="code">
          <n-input v-model:value="form.code" placeholder="WO-2026-0001" />
        </n-form-item>
        <n-form-item label="标题" path="title">
          <n-input v-model:value="form.title" placeholder="可选" type="textarea" :autosize="{ minRows: 2, maxRows: 4 }" />
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showForm = false">取消</n-button>
          <n-button type="primary" :loading="submitting" @click="submit">
            <span v-text="editingId ? '保存' : '创建'" />
          </n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- Upload drawer -->
    <n-drawer v-model:show="showUpload" :width="560" placement="right">
      <n-drawer-content :title="uploadTitle" :native-scrollbar="false" closable>
        <upload-dropzone
          v-if="showUpload && uploadTarget"
          :project-id="projectId"
          :wo-id="uploadTarget.id"
          @uploaded="onUploaded"
        />
      </n-drawer-content>
    </n-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, h, onMounted, reactive, ref } from "vue"
import {
  NButton,
  NDataTable,
  NDrawer,
  NDrawerContent,
  NForm,
  NFormItem,
  NInput,
  NModal,
  NPagination,
  NSelect,
  NSpace,
  NTag,
  useDialog,
  useMessage,
  type DataTableColumns,
  type FormInst,
  type FormRules,
  type SelectOption,
} from "naive-ui"
import { useAuthStore } from "@/stores/auth"
import {
  WO_STATUS_LABELS,
  WO_STATUS_TAG_TYPE,
  allowedTransitions,
  workOrdersApi,
} from "@/api/work_orders"
import type { WorkOrder, WorkOrderStatus } from "@/api/types"
import UploadDropzone from "@/components/UploadDropzone.vue"

// Default project (per env doc; multi-project routing comes later).
const DEFAULT_PROJECT_ID = "00000000-0000-0000-0000-000000000001"
const projectId = DEFAULT_PROJECT_ID

const auth = useAuthStore()
const message = useMessage()
const dialog = useDialog()
const api = workOrdersApi(projectId)

// Members can list/upload but cannot create/patch/delete WOs (server requires upload perm).
// We gate write actions behind admin only here for simplicity; backend re-checks.
const canManage = computed(() => auth.isAdmin)
const canUpload = computed(() => auth.isAdmin)

const statusOptions: SelectOption[] = [
  { label: "待开工", value: "open" },
  { label: "进行中", value: "in_progress" },
  { label: "已完成", value: "done" },
  { label: "已取消", value: "cancelled" },
]

const rows = ref<WorkOrder[]>([])
const total = ref(0)
const loading = ref(false)
const page = ref(1)
const pageSize = ref(20)
const q = ref("")
const statusFilter = ref<WorkOrderStatus | null>(null)

async function reload() {
  loading.value = true
  try {
    const res = await api.list({
      q: q.value || undefined,
      status: statusFilter.value || undefined,
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

// --- Create / Edit ---
const showForm = ref(false)
const submitting = ref(false)
const editingId = ref<string | null>(null)
const formRef = ref<FormInst | null>(null)
const form = reactive({
  code: "",
  title: "",
})
const rules: FormRules = {
  code: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
}

function openCreate() {
  editingId.value = null
  form.code = ""
  form.title = ""
  showForm.value = true
}

function openEdit(wo: WorkOrder) {
  editingId.value = wo.id
  form.code = wo.code
  form.title = wo.title || ""
  showForm.value = true
}

async function submit() {
  if (!formRef.value) return
  await formRef.value.validate()
  submitting.value = true
  try {
    const title = form.title.trim() || null
    if (editingId.value) {
      await api.patch(editingId.value, { code: form.code.trim(), title })
      message.success("已保存")
    } else {
      await api.create({ code: form.code.trim(), title })
      message.success("已创建")
    }
    showForm.value = false
    reload()
  } finally {
    submitting.value = false
  }
}

// --- Transition ---
async function transitionTo(wo: WorkOrder, to: WorkOrderStatus) {
  await api.transition(wo.id, to)
  message.success(`状态 -> ${WO_STATUS_LABELS[to]}`)
  reload()
}

// --- Delete ---
function confirmDelete(wo: WorkOrder) {
  dialog.warning({
    title: "删除工单",
    content: `确认删除「${wo.code}」？此操作不可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      await api.remove(wo.id)
      message.success("已删除")
      reload()
    },
  })
}

// --- Upload ---
const showUpload = ref(false)
const uploadTarget = ref<WorkOrder | null>(null)
const uploadTitle = computed(() =>
  uploadTarget.value ? `上传照片 · ${uploadTarget.value.code}` : "上传照片",
)

function openUpload(wo: WorkOrder) {
  uploadTarget.value = wo
  showUpload.value = true
}

function onUploaded() {
  // Future: jump to recognition view; for now just notify.
}

// --- Columns ---
const columns: DataTableColumns<WorkOrder> = [
  { title: "工单号", key: "code", width: 180 },
  {
    title: "标题",
    key: "title",
    render: (r) => r.title || "-",
  },
  {
    title: "状态",
    key: "status",
    width: 110,
    render: (r) => {
      const s = (r.status as WorkOrderStatus) in WO_STATUS_LABELS ? (r.status as WorkOrderStatus) : null
      const label = s ? WO_STATUS_LABELS[s] : r.status
      const type = s ? WO_STATUS_TAG_TYPE[s] : "default"
      return h(NTag, { type, size: "small" }, () => label)
    },
  },
  {
    title: "创建时间",
    key: "created_at",
    width: 170,
    render: (r) => new Date(r.created_at).toLocaleString(),
  },
  {
    title: "操作",
    key: "actions",
    width: 380,
    render: (r) => {
      const transitions = allowedTransitions(r.status as string)
      const items: ReturnType<typeof h>[] = []
      if (canUpload.value) {
        items.push(
          h(
            NButton,
            { size: "tiny", type: "primary", onClick: () => openUpload(r) },
            () => "上传照片",
          ),
        )
      }
      if (canManage.value) {
        items.push(
          h(NButton, { size: "tiny", onClick: () => openEdit(r) }, () => "编辑"),
        )
        for (const to of transitions) {
          items.push(
            h(
              NButton,
              { size: "tiny", onClick: () => transitionTo(r, to) },
              () => `→ ${WO_STATUS_LABELS[to]}`,
            ),
          )
        }
        items.push(
          h(
            NButton,
            { size: "tiny", type: "warning", onClick: () => confirmDelete(r) },
            () => "删除",
          ),
        )
      }
      return h(NSpace, { size: "small" }, () => items)
    },
  },
]
</script>
