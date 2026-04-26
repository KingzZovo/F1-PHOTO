<template>
  <div class="p-6 space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-2xl font-semibold">设备</h1>
      <n-space>
        <n-input
          v-model:value="md.query.q"
          placeholder="搜索 SN/名称/型号"
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
        <n-button v-if="auth.isAdmin" type="primary" @click="openCreate">+ 新增设备</n-button>
      </n-space>
    </div>

    <n-data-table
      :columns="columns"
      :data="md.visibleRows.value"
      :loading="md.loading.value"
      :row-key="(r: Device) => r.id"
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

    <n-modal v-model:show="showForm" preset="card" :title="editingId ? '编辑设备' : '新增设备'" style="max-width: 480px" :mask-closable="false">
      <n-form ref="formRef" :model="form" :rules="rules" label-placement="left" label-width="80" require-mark-placement="right-hanging">
        <n-form-item label="SN" path="sn">
          <n-input v-model:value="form.sn" placeholder="D-001" />
        </n-form-item>
        <n-form-item label="名称" path="name">
          <n-input v-model:value="form.name" />
        </n-form-item>
        <n-form-item label="型号" path="model">
          <n-input v-model:value="form.model" placeholder="可选" />
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
import { devicesApi } from "@/api/master"
import type { Device, DeviceInput, DevicePatch } from "@/api/types"

const auth = useAuthStore()
const message = useMessage()
const dialog = useDialog()

const md = useMasterData<Device, DeviceInput, DevicePatch>(devicesApi)

const showForm = ref(false)
const submitting = ref(false)
const editingId = ref<string | null>(null)
const formRef = ref<FormInst | null>(null)
const form = reactive({
  sn: "",
  name: "",
  model: "",
})
const rules: FormRules = {
  sn: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
  name: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
}

function openCreate() {
  editingId.value = null
  form.sn = ""
  form.name = ""
  form.model = ""
  showForm.value = true
}

function openEdit(d: Device) {
  editingId.value = d.id
  form.sn = d.sn
  form.name = d.name
  form.model = d.model || ""
  showForm.value = true
}

async function submit() {
  if (!formRef.value) return
  await formRef.value.validate()
  submitting.value = true
  try {
    const m = form.model.trim() || null
    if (editingId.value) {
      await md.patch(editingId.value, {
        sn: form.sn.trim(),
        name: form.name.trim(),
        model: m,
      })
      message.success("已保存")
    } else {
      await md.create({
        sn: form.sn.trim(),
        name: form.name.trim(),
        model: m,
      })
      message.success("已创建")
    }
    showForm.value = false
  } finally {
    submitting.value = false
  }
}

function confirmDelete(d: Device) {
  dialog.warning({
    title: "删除设备",
    content: `确认软删除「${d.name}（${d.sn}）」？删除后可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      await md.softDelete(d.id)
      message.success("已删除")
    },
  })
}

async function onRestore(d: Device) {
  await md.restore(d.id)
  message.success("已恢复")
}

const columns: DataTableColumns<Device> = [
  { title: "SN", key: "sn" },
  { title: "名称", key: "name" },
  { title: "型号", key: "model", render: (r) => r.model || "-" },
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
