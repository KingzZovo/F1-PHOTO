<template>
  <div class="p-6 space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-2xl font-semibold">工具</h1>
      <n-space>
        <n-input
          v-model:value="md.query.q"
          placeholder="搜索 SN/名称/类别"
          clearable
          style="width: 280px"
          @keyup.enter="md.onSearch"
          @clear="md.onSearch"
        />
        <n-button @click="md.onSearch">搜索</n-button>
        <n-radio-group v-model:value="md.archivedFilter.value" @update:value="md.onArchivedChange">
          <n-radio-button value="active">活跃</n-radio-button>
          <n-radio-button value="archived">已删除</n-radio-button>
          <n-radio-button value="all">全部</n-radio-button>
        </n-radio-group>
        <n-button v-if="auth.isAdmin" type="primary" @click="openCreate">+ 新增工具</n-button>
      </n-space>
    </div>

    <n-data-table
      :columns="columns"
      :data="md.visibleRows.value"
      :loading="md.loading.value"
      :row-key="(r: Tool) => r.id"
      :bordered="false"
    />

    <div class="flex justify-end">
      <n-pagination
        v-model:page="md.query.page"
        v-model:page-size="md.query.page_size"
        :item-count="md.total.value"
        :page-sizes="[10, 20, 50, 100]"
        show-size-picker
        @update:page="md.onPageChange"
        @update:page-size="md.onPageSizeChange"
      />
    </div>

    <n-modal v-model:show="showForm" preset="card" :title="editingId ? '编辑工具' : '新增工具'" style="max-width: 480px" :mask-closable="false">
      <n-form ref="formRef" :model="form" :rules="rules" label-placement="left" label-width="80" require-mark-placement="right-hanging">
        <n-form-item label="SN" path="sn">
          <n-input v-model:value="form.sn" placeholder="T-001" />
        </n-form-item>
        <n-form-item label="名称" path="name">
          <n-input v-model:value="form.name" />
        </n-form-item>
        <n-form-item label="类别" path="category">
          <n-input v-model:value="form.category" placeholder="可选" />
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
  </div>
</template>

<script setup lang="ts">
import { h, reactive, ref } from "vue"
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
  NSpace,
  NTag,
  useDialog,
  useMessage,
  type DataTableColumns,
  type FormInst,
  type FormRules,
} from "naive-ui"
import { useAuthStore } from "@/stores/auth"
import { useMasterData } from "@/composables/useMasterData"
import { toolsApi } from "@/api/master"
import type { Tool, ToolInput, ToolPatch } from "@/api/types"

const auth = useAuthStore()
const message = useMessage()
const dialog = useDialog()

const md = useMasterData<Tool, ToolInput, ToolPatch>(toolsApi)

const showForm = ref(false)
const submitting = ref(false)
const editingId = ref<string | null>(null)
const formRef = ref<FormInst | null>(null)
const form = reactive({
  sn: "",
  name: "",
  category: "",
})
const rules: FormRules = {
  sn: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
  name: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
}

function openCreate() {
  editingId.value = null
  form.sn = ""
  form.name = ""
  form.category = ""
  showForm.value = true
}

function openEdit(t: Tool) {
  editingId.value = t.id
  form.sn = t.sn
  form.name = t.name
  form.category = t.category || ""
  showForm.value = true
}

async function submit() {
  if (!formRef.value) return
  await formRef.value.validate()
  submitting.value = true
  try {
    const cat = form.category.trim() || null
    if (editingId.value) {
      await md.patch(editingId.value, {
        sn: form.sn.trim(),
        name: form.name.trim(),
        category: cat,
      })
      message.success("已保存")
    } else {
      await md.create({
        sn: form.sn.trim(),
        name: form.name.trim(),
        category: cat,
      })
      message.success("已创建")
    }
    showForm.value = false
  } finally {
    submitting.value = false
  }
}

function confirmDelete(t: Tool) {
  dialog.warning({
    title: "删除工具",
    content: `确认软删除「${t.name}（${t.sn}）」？删除后可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      await md.softDelete(t.id)
      message.success("已删除")
    },
  })
}

async function onRestore(t: Tool) {
  await md.restore(t.id)
  message.success("已恢复")
}

const columns: DataTableColumns<Tool> = [
  { title: "SN", key: "sn" },
  { title: "名称", key: "name" },
  { title: "类别", key: "category", render: (r) => r.category || "-" },
  { title: "照片数", key: "photo_count", width: 90 },
  {
    title: "状态",
    key: "deleted_at",
    width: 90,
    render: (r) =>
      r.deleted_at
        ? h(NTag, { type: "warning", size: "small" }, () => "已删除")
        : h(NTag, { type: "success", size: "small" }, () => "活跃"),
  },
  {
    title: "创建时间",
    key: "created_at",
    render: (r) => new Date(r.created_at).toLocaleString(),
  },
  {
    title: "操作",
    key: "actions",
    width: 200,
    render: (r) =>
      h(NSpace, { size: "small" }, () => [
        auth.isAdmin && !r.deleted_at
          ? h(NButton, { size: "tiny", onClick: () => openEdit(r) }, () => "编辑")
          : null,
        auth.isAdmin && !r.deleted_at
          ? h(NButton, { size: "tiny", type: "warning", onClick: () => confirmDelete(r) }, () => "删除")
          : null,
        auth.isAdmin && r.deleted_at
          ? h(NButton, { size: "tiny", type: "primary", onClick: () => onRestore(r) }, () => "恢复")
          : null,
      ]),
  },
]
</script>
