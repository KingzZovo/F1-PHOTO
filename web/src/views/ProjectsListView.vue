<template>
  <div class="p-6 space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-2xl font-semibold">项目</h1>
      <n-space>
        <n-radio-group v-model:value="archivedFilter" @update:value="reload">
          <n-radio-button value="active">活跃</n-radio-button>
          <n-radio-button value="archived">已归档</n-radio-button>
          <n-radio-button value="all">全部</n-radio-button>
        </n-radio-group>
        <n-button v-if="auth.isAdmin" type="primary" @click="openCreate">
          + 新建项目
        </n-button>
      </n-space>
    </div>

    <n-data-table
      :columns="columns"
      :data="projects"
      :loading="loading"
      :row-key="(r: Project) => r.id"
      :bordered="false"
    />

    <n-modal v-model:show="showCreate" preset="card" title="新建项目" style="max-width: 480px" :mask-closable="false">
      <n-form ref="createFormRef" :model="createForm" :rules="createRules" label-placement="left" label-width="80" require-mark-placement="right-hanging">
        <n-form-item label="Code" path="code"><n-input v-model:value="createForm.code" placeholder="唯一标识，例 project-a" /></n-form-item>
        <n-form-item label="名称" path="name"><n-input v-model:value="createForm.name" /></n-form-item>
        <n-form-item label="图标" path="icon"><n-input v-model:value="createForm.icon" placeholder="emoji 或 URL（可选）" /></n-form-item>
        <n-form-item label="描述" path="description"><n-input v-model:value="createForm.description" type="textarea" :autosize="{minRows: 2, maxRows: 5}" /></n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showCreate = false">取消</n-button>
          <n-button type="primary" :loading="creating" @click="submitCreate">创建</n-button>
        </n-space>
      </template>
    </n-modal>
  </div>
</template>

<script setup lang="ts">
import { h, onMounted, reactive, ref } from "vue"
import { useRouter } from "vue-router"
import {
  NButton,
  NDataTable,
  NForm,
  NFormItem,
  NInput,
  NModal,
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
import { projectsApi, type ArchivedFilter } from "@/api/projects"
import { useAuthStore } from "@/stores/auth"
import type { Project } from "@/api/types"

const auth = useAuthStore()
const router = useRouter()
const message = useMessage()
const dialog = useDialog()

const projects = ref<Project[]>([])
const loading = ref(false)
const archivedFilter = ref<ArchivedFilter>("active")

const showCreate = ref(false)
const creating = ref(false)
const createFormRef = ref<FormInst | null>(null)
const createForm = reactive({ code: "", name: "", icon: "", description: "" })
const createRules: FormRules = {
  code: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
  name: [{ required: true, message: "必填", trigger: ["blur", "input"] }],
}

async function reload() {
  loading.value = true
  try {
    projects.value = await projectsApi.list(archivedFilter.value)
  } catch (_e) {
    /* axios interceptor toasts already */
  } finally {
    loading.value = false
  }
}

function openCreate() {
  createForm.code = ""
  createForm.name = ""
  createForm.icon = ""
  createForm.description = ""
  showCreate.value = true
}

async function submitCreate() {
  if (!createFormRef.value) return
  await createFormRef.value.validate()
  creating.value = true
  try {
    const proj = await projectsApi.create({
      code: createForm.code.trim(),
      name: createForm.name.trim(),
      icon: createForm.icon.trim() || null,
      description: createForm.description.trim() || null,
    })
    message.success(`已创建 ${proj.code}`)
    showCreate.value = false
    await reload()
  } finally {
    creating.value = false
  }
}

function confirmArchive(p: Project) {
  dialog.warning({
    title: "归档项目",
    content: `确认归档 「${p.code}」？归档后仅 admin 可见。`,
    positiveText: "归档",
    negativeText: "取消",
    onPositiveClick: async () => {
      await projectsApi.archive(p.id)
      message.success(`已归档 ${p.code}`)
      await reload()
    },
  })
}

async function unarchive(p: Project) {
  await projectsApi.unarchive(p.id)
  message.success(`已恢复 ${p.code}`)
  await reload()
}

const columns: DataTableColumns<Project> = [
  {
    title: "Code",
    key: "code",
    render: (r) => h(NButton, { text: true, type: "primary", onClick: () => router.push({ name: "project-detail", params: { id: r.id } }) }, () => r.code),
  },
  { title: "名称", key: "name" },
  {
    title: "状态",
    key: "archived_at",
    render: (r) => r.archived_at
      ? h(NTag, { type: "warning", size: "small" }, () => "归档")
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
    render: (r) => h(NSpace, { size: "small" }, () => [
      h(NButton, { size: "tiny", onClick: () => router.push({ name: "project-detail", params: { id: r.id } }) }, () => "查看"),
      auth.isAdmin && !r.archived_at
        ? h(NButton, { size: "tiny", type: "warning", onClick: () => confirmArchive(r) }, () => "归档")
        : null,
      auth.isAdmin && r.archived_at
        ? h(NButton, { size: "tiny", type: "primary", onClick: () => unarchive(r) }, () => "恢复")
        : null,
    ]),
  },
]

onMounted(reload)
</script>
