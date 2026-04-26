<template>
  <div class="space-y-3">
    <n-form label-placement="left" label-width="90" size="small">
      <n-form-item label="所属类型">
        <n-radio-group v-model:value="ownerType">
          <n-radio-button value="person">人员</n-radio-button>
          <n-radio-button value="tool">工具</n-radio-button>
          <n-radio-button value="device">设备</n-radio-button>
          <n-radio-button value="wo_raw">现场原图</n-radio-button>
        </n-radio-group>
      </n-form-item>
      <n-form-item v-if="ownerType === 'person'" label="工号提示">
        <n-input v-model:value="employeeNo" placeholder="可选：如 E001，用于唤起已有人员" clearable />
      </n-form-item>
      <n-form-item v-if="ownerType === 'tool' || ownerType === 'device'" label="SN 提示">
        <n-input v-model:value="sn" :placeholder="ownerType === 'tool' ? '可选：如 T-001' : '可选：如 D-001'" clearable />
      </n-form-item>
      <n-form-item label="拍摄角度">
        <n-radio-group v-model:value="angle">
          <n-radio-button value="unknown">未知</n-radio-button>
          <n-radio-button value="front">正面</n-radio-button>
          <n-radio-button value="side">侧面</n-radio-button>
          <n-radio-button value="back">背面</n-radio-button>
        </n-radio-group>
      </n-form-item>
    </n-form>

    <n-upload
      multiple
      directory-dnd
      :default-upload="false"
      :show-remove-button="true"
      :show-cancel-button="false"
      :file-list="fileList"
      accept="image/*"
      list-type="image-card"
      @update:file-list="onFileListChange"
    >
      <n-upload-dragger>
        <div class="py-4 text-center">
          <div class="mb-1 text-base">拖拽图片到这里，或点击选择</div>
          <div class="text-xs text-gray-500">支持多选。仅接受图片。</div>
        </div>
      </n-upload-dragger>
    </n-upload>

    <div v-if="summary" class="text-sm text-gray-600">
      <span v-text="summary" />
    </div>

    <div class="flex justify-end gap-2">
      <n-button :disabled="uploading" @click="clearAll">清空</n-button>
      <n-button type="primary" :loading="uploading" :disabled="!canUpload" @click="startUpload">
        <span v-text="uploadButtonLabel" />
      </n-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from "vue"
import {
  NButton,
  NForm,
  NFormItem,
  NInput,
  NRadioButton,
  NRadioGroup,
  NUpload,
  NUploadDragger,
  useMessage,
  type UploadFileInfo,
} from "naive-ui"
import { photosApi } from "@/api/photos"
import type { AngleKind, OwnerType, PhotoUploadResponse } from "@/api/types"

const props = defineProps<{
  projectId: string
  /** Pass either woId (preferred) or woCode. */
  woId?: string
  woCode?: string
}>()

const emit = defineEmits<{
  (e: "uploaded", batch: PhotoUploadResponse[]): void
}>()

const message = useMessage()
const api = computed(() => photosApi(props.projectId))

const ownerType = ref<OwnerType>("wo_raw")
const employeeNo = ref("")
const sn = ref("")
const angle = ref<AngleKind>("unknown")
const fileList = ref<UploadFileInfo[]>([])
const uploading = ref(false)

watch(ownerType, () => {
  // Reset hints to avoid sending stale fields when switching owner types.
  if (ownerType.value !== "person") employeeNo.value = ""
  if (ownerType.value !== "tool" && ownerType.value !== "device") sn.value = ""
})

function onFileListChange(list: UploadFileInfo[]) {
  fileList.value = list
}

const pendingFiles = computed(() =>
  fileList.value.filter((f) => f.status === "pending" || f.status === "error"),
)

const summary = computed(() => {
  if (fileList.value.length === 0) return ""
  const total = fileList.value.length
  const ok = fileList.value.filter((f) => f.status === "finished").length
  const fail = fileList.value.filter((f) => f.status === "error").length
  const pending = total - ok - fail
  return `共 ${total} 张：成功 ${ok}，失败 ${fail}，待上传 ${pending}`
})

const canUpload = computed(
  () =>
    !uploading.value && pendingFiles.value.length > 0 && (props.woId || props.woCode || false),
)

const uploadButtonLabel = computed(() =>
  uploading.value ? "上传中…" : `上传 ${pendingFiles.value.length} 张`,
)

function clearAll() {
  fileList.value = []
}

async function startUpload() {
  if (!canUpload.value) return
  uploading.value = true
  const responses: PhotoUploadResponse[] = []
  try {
    for (const item of pendingFiles.value) {
      const file = item.file
      if (!file) continue
      item.status = "uploading"
      try {
        const resp = await api.value.upload({
          file,
          owner_type: ownerType.value,
          wo_id: props.woId,
          wo_code: props.woCode,
          employee_no: ownerType.value === "person" ? employeeNo.value.trim() || undefined : undefined,
          sn:
            ownerType.value === "tool" || ownerType.value === "device"
              ? sn.value.trim() || undefined
              : undefined,
          angle: angle.value,
        })
        item.status = "finished"
        responses.push(resp)
      } catch (err) {
        item.status = "error"
      }
    }
    if (responses.length > 0) {
      emit("uploaded", responses)
      message.success(`已上传 ${responses.length} 张，已入识别队列`)
    }
    if (responses.length < pendingFiles.value.length) {
      const failed = fileList.value.filter((f) => f.status === "error").length
      if (failed > 0) message.error(`${failed} 张上传失败`)
    }
  } finally {
    uploading.value = false
  }
}
</script>
