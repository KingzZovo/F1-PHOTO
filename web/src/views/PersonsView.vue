<template>
  <div class="p-6 space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-2xl font-semibold">人员</h1>
      <n-space>
        <n-input
          v-model:value="md.query.q"
          placeholder="搜索 工号/姓名/部门/电话"
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
        <n-button v-if="auth.isAdmin" type="primary" @click="openCreate">+ 新增人员</n-button>
      </n-space>
    </div>

    <n-data-table
      :columns="columns"
      :data="md.visibleRows.value"
      :loading="md.loading.value"
      :row-key="(r: Person) => r.id"
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

    <n-modal v-model:show="showForm" preset="card" :title="editingId ? '编辑人员' : '新增人员'" style="max-width: 480px" :mask-closable="false">
      <n-form ref="formRef" :model="form" :rules="rules" label-placement="left" label-width="80" require-mark-placement="right-hanging">
        <n-form-item label="工号" path="employee_no">
          <n-input v-model:value="form.employee_no" placeholder="E001" />
        </n-form-item>
        <n-form-item label="姓名" path="name">
          <n-input v-model:value="form.name" />
        </n-form-item>
        <n-form-item label="部门" path="department">
          <n-input v-model:value="form.department" placeholder="可选" />
        </n-form-item>
        <n-form-item label="电话" path="phone">
          <n-input v-model:value="form.phone" placeholder="可选" />
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
import { personsApi } from "@/api/master"
import type { Person, PersonInput, PersonPatch } from "@/api/types"

const auth = useAuthStore()
const message = useMessage()
const dialog = useDialog()

const md = useMasterData<Person, PersonInput, PersonPatch>(personsApi)

const showForm = ref(false)
const submitting = ref(false)
const editingId = ref<string | null>(null)
const formRef = ref<FormInst | null>(null)
const form = reactive({
  employee_no: "",
  name: "",
  department: "",
  phone: "",
})
const rules: FormRules = {
  employee_no: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
  name: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
}

function openCreate() {
  editingId.value = null
  form.employee_no = ""
  form.name = ""
  form.department = ""
  form.phone = ""
  showForm.value = true
}

function openEdit(p: Person) {
  editingId.value = p.id
  form.employee_no = p.employee_no
  form.name = p.name
  form.department = p.department || ""
  form.phone = p.phone || ""
  showForm.value = true
}

async function submit() {
  if (!formRef.value) return
  await formRef.value.validate()
  submitting.value = true
  try {
    const dept = form.department.trim() || null
    const phone = form.phone.trim() || null
    if (editingId.value) {
      await md.patch(editingId.value, {
        employee_no: form.employee_no.trim(),
        name: form.name.trim(),
        department: dept,
        phone,
      })
      message.success("已保存")
    } else {
      await md.create({
        employee_no: form.employee_no.trim(),
        name: form.name.trim(),
        department: dept,
        phone,
      })
      message.success("已创建")
    }
    showForm.value = false
  } finally {
    submitting.value = false
  }
}

function confirmDelete(p: Person) {
  dialog.warning({
    title: "删除人员",
    content: `确认软删除「${p.name}（${p.employee_no}）」？删除后可恢复。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      await md.softDelete(p.id)
      message.success("已删除")
    },
  })
}

async function onRestore(p: Person) {
  await md.restore(p.id)
  message.success("已恢复")
}

const columns: DataTableColumns<Person> = [
  { title: "工号", key: "employee_no" },
  { title: "姓名", key: "name" },
  { title: "部门", key: "department", render: (r) => r.department || "-" },
  { title: "电话", key: "phone", render: (r) => r.phone || "-" },
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
