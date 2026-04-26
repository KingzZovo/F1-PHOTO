<template>
  <div class="p-6 space-y-4">
    <n-page-header @back="() => router.push({ name: 'projects' })">
      <template #title>
        <span class="text-xl font-semibold">
          <span v-if="project?.icon">{ project.icon }&nbsp;</span>
          <span v-text="project?.name || '加载中...'" />
        </span>
      </template>
      <template #subtitle>
        <span class="text-gray-500" v-text="project?.code" />
      </template>
      <template #extra>
        <n-space>
          <n-tag v-if="project?.archived_at" type="warning" size="small">已归档</n-tag>
          <n-tag v-else type="success" size="small">活跃</n-tag>
          <n-button v-if="auth.isAdmin && !project?.archived_at" @click="openEdit">编辑</n-button>
        </n-space>
      </template>
    </n-page-header>

    <n-tabs v-model:value="tab" type="line" animated>
      <n-tab-pane name="overview" tab="概览">
        <n-descriptions :column="2" label-placement="left" bordered class="max-w-3xl">
          <n-descriptions-item label="Code"><span v-text="project?.code" /></n-descriptions-item>
          <n-descriptions-item label="名称"><span v-text="project?.name" /></n-descriptions-item>
          <n-descriptions-item label="图标"><span v-text="project?.icon || '—'" /></n-descriptions-item>
          <n-descriptions-item label="创建时间"><span v-text="project ? new Date(project.created_at).toLocaleString() : ''" /></n-descriptions-item>
          <n-descriptions-item label="描述" :span="2"><span v-text="project?.description || '—'" /></n-descriptions-item>
        </n-descriptions>
        <n-card v-if="perms" title="我的权限" size="small" class="mt-4 max-w-3xl">
          <n-space>
            <n-tag :type="perms.is_admin ? 'success' : 'default'">admin</n-tag>
            <n-tag :type="perms.can_view ? 'success' : 'default'">view</n-tag>
            <n-tag :type="perms.can_upload ? 'success' : 'default'">upload</n-tag>
            <n-tag :type="perms.can_delete ? 'success' : 'default'">delete</n-tag>
            <n-tag :type="perms.can_manage ? 'success' : 'default'">manage</n-tag>
          </n-space>
        </n-card>
      </n-tab-pane>

      <n-tab-pane name="members" tab="成员">
        <div class="flex items-center justify-end mb-3">
          <n-button v-if="canManage" type="primary" @click="openAdd">+ 添加成员</n-button>
        </div>
        <n-data-table :columns="memberColumns" :data="members" :loading="loadingMembers" :row-key="(r: Member) => r.user_id" />
      </n-tab-pane>
    </n-tabs>

    <!-- Edit project modal -->
    <n-modal v-model:show="showEdit" preset="card" title="编辑项目" style="max-width: 480px" :mask-closable="false">
      <n-form :model="editForm" label-placement="left" label-width="80">
        <n-form-item label="名称"><n-input v-model:value="editForm.name" /></n-form-item>
        <n-form-item label="图标"><n-input v-model:value="editForm.icon" /></n-form-item>
        <n-form-item label="描述"><n-input v-model:value="editForm.description" type="textarea" :autosize="{minRows: 2, maxRows: 5}" /></n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showEdit = false">取消</n-button>
          <n-button type="primary" :loading="savingEdit" @click="submitEdit">保存</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- Add / edit member modal -->
    <n-modal v-model:show="showMemberModal" preset="card" :title="memberMode === 'add' ? '添加成员' : '编辑权限'" style="max-width: 460px" :mask-closable="false">
      <n-form :model="memberForm" label-placement="left" label-width="80">
        <n-form-item v-if="memberMode === 'add'" label="用户名">
          <n-input v-model:value="memberForm.username" placeholder="系统中已存在的账号名" />
        </n-form-item>
        <n-form-item v-else label="用户名">
          <span v-text="memberForm.username" />
        </n-form-item>
        <n-form-item label="view"><n-checkbox v-model:checked="memberForm.can_view" /></n-form-item>
        <n-form-item label="upload"><n-checkbox v-model:checked="memberForm.can_upload" /></n-form-item>
        <n-form-item label="delete"><n-checkbox v-model:checked="memberForm.can_delete" /></n-form-item>
        <n-form-item label="manage"><n-checkbox v-model:checked="memberForm.can_manage" /></n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="showMemberModal = false">取消</n-button>
          <n-button type="primary" :loading="savingMember" @click="submitMember">保存</n-button>
        </n-space>
      </template>
    </n-modal>
  </div>
</template>

<script setup lang="ts">
import { computed, h, onMounted, reactive, ref, watch } from "vue"
import { useRoute, useRouter } from "vue-router"
import {
  NButton,
  NCard,
  NCheckbox,
  NDataTable,
  NDescriptions,
  NDescriptionsItem,
  NForm,
  NFormItem,
  NInput,
  NModal,
  NPageHeader,
  NSpace,
  NTabPane,
  NTabs,
  NTag,
  useDialog,
  useMessage,
  type DataTableColumns,
} from "naive-ui"
import { projectsApi } from "@/api/projects"
import { useAuthStore } from "@/stores/auth"
import type { Member, MyPerms, Project } from "@/api/types"

const route = useRoute()
const router = useRouter()
const auth = useAuthStore()
const message = useMessage()
const dialog = useDialog()

const projectId = computed(() => route.params.id as string)
const project = ref<Project | null>(null)
const perms = ref<MyPerms | null>(null)
const members = ref<Member[]>([])
const loadingMembers = ref(false)
const tab = ref<"overview" | "members">("overview")

const canManage = computed(() => !!perms.value && (perms.value.is_admin || perms.value.can_manage))

const showEdit = ref(false)
const savingEdit = ref(false)
const editForm = reactive({ name: "", icon: "", description: "" })

const showMemberModal = ref(false)
const savingMember = ref(false)
const memberMode = ref<"add" | "edit">("add")
const memberForm = reactive({
  user_id: "",
  username: "",
  can_view: true,
  can_upload: false,
  can_delete: false,
  can_manage: false,
})

async function loadProject() {
  project.value = await projectsApi.get(projectId.value)
  perms.value = await projectsApi.myPerms(projectId.value)
}

async function loadMembers() {
  loadingMembers.value = true
  try {
    members.value = await projectsApi.listMembers(projectId.value)
  } finally {
    loadingMembers.value = false
  }
}

function openEdit() {
  if (!project.value) return
  editForm.name = project.value.name
  editForm.icon = project.value.icon || ""
  editForm.description = project.value.description || ""
  showEdit.value = true
}

async function submitEdit() {
  savingEdit.value = true
  try {
    project.value = await projectsApi.patch(projectId.value, {
      name: editForm.name.trim(),
      icon: editForm.icon.trim() || null,
      description: editForm.description.trim() || null,
    })
    message.success("已保存")
    showEdit.value = false
  } finally {
    savingEdit.value = false
  }
}

function openAdd() {
  memberMode.value = "add"
  memberForm.user_id = ""
  memberForm.username = ""
  memberForm.can_view = true
  memberForm.can_upload = false
  memberForm.can_delete = false
  memberForm.can_manage = false
  showMemberModal.value = true
}

function openEditMember(m: Member) {
  memberMode.value = "edit"
  memberForm.user_id = m.user_id
  memberForm.username = m.username
  memberForm.can_view = m.can_view
  memberForm.can_upload = m.can_upload
  memberForm.can_delete = m.can_delete
  memberForm.can_manage = m.can_manage
  showMemberModal.value = true
}

async function submitMember() {
  savingMember.value = true
  try {
    if (memberMode.value === "add") {
      await projectsApi.addMember(projectId.value, {
        username: memberForm.username.trim(),
        can_view: memberForm.can_view,
        can_upload: memberForm.can_upload,
        can_delete: memberForm.can_delete,
        can_manage: memberForm.can_manage,
      })
      message.success("已添加")
    } else {
      await projectsApi.patchMember(projectId.value, memberForm.user_id, {
        can_view: memberForm.can_view,
        can_upload: memberForm.can_upload,
        can_delete: memberForm.can_delete,
        can_manage: memberForm.can_manage,
      })
      message.success("已更新")
    }
    showMemberModal.value = false
    await loadMembers()
  } finally {
    savingMember.value = false
  }
}

function confirmRemove(m: Member) {
  dialog.warning({
    title: "移除成员",
    content: `确认从项目中移除 「${m.username}」？`,
    positiveText: "移除",
    negativeText: "取消",
    onPositiveClick: async () => {
      await projectsApi.removeMember(projectId.value, m.user_id)
      message.success("已移除")
      await loadMembers()
    },
  })
}

const memberColumns: DataTableColumns<Member> = [
  { title: "用户", key: "username" },
  { title: "姓名", key: "full_name", render: (r) => r.full_name || "—" },
  { title: "角色", key: "role" },
  {
    title: "权限",
    key: "perms",
    render: (r) => h(NSpace, { size: "small" }, () => [
      r.can_view ? h(NTag, { type: "success", size: "small" }, () => "view") : null,
      r.can_upload ? h(NTag, { type: "info", size: "small" }, () => "upload") : null,
      r.can_delete ? h(NTag, { type: "warning", size: "small" }, () => "delete") : null,
      r.can_manage ? h(NTag, { type: "error", size: "small" }, () => "manage") : null,
    ]),
  },
  {
    title: "加入时间",
    key: "created_at",
    render: (r) => new Date(r.created_at).toLocaleString(),
  },
  {
    title: "操作",
    key: "actions",
    render: (r) => canManage.value
      ? h(NSpace, { size: "small" }, () => [
          h(NButton, { size: "tiny", onClick: () => openEditMember(r) }, () => "编辑"),
          h(NButton, { size: "tiny", type: "error", onClick: () => confirmRemove(r) }, () => "移除"),
        ])
      : null,
  },
]

watch(tab, (v) => {
  if (v === "members" && members.value.length === 0) loadMembers()
})

onMounted(async () => {
  await loadProject()
  await loadMembers()
})
</script>
