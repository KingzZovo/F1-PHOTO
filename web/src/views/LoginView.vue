<template>
  <div class="min-h-screen flex items-center justify-center px-4">
    <n-card class="w-full max-w-sm shadow-md" title="F1-photo 登录">
      <n-form ref="formRef" :model="form" :rules="rules" label-placement="top" @keyup.enter="submit">
        <n-form-item label="用户名" path="username">
          <n-input v-model:value="form.username" placeholder="admin" autofocus />
        </n-form-item>
        <n-form-item label="密码" path="password">
          <n-input v-model:value="form.password" type="password" show-password-on="click" placeholder="********" />
        </n-form-item>
        <n-button type="primary" block :loading="loading" @click="submit">登录</n-button>
      </n-form>
      <p class="text-xs text-gray-500 mt-4 text-center">F1-photo · 工单照片归档系统</p>
    </n-card>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref } from "vue"
import { useRouter, useRoute } from "vue-router"
import { NCard, NForm, NFormItem, NInput, NButton, useMessage, type FormInst, type FormRules } from "naive-ui"
import { useAuthStore } from "@/stores/auth"
import { ApiError } from "@/api/client"

const router = useRouter()
const route = useRoute()
const auth = useAuthStore()
const message = useMessage()

const formRef = ref<FormInst | null>(null)
const form = reactive({ username: "", password: "" })
const loading = ref(false)

const rules: FormRules = {
  username: [{ required: true, message: "请输入用户名", trigger: ["blur", "input"] }],
  password: [{ required: true, message: "请输入密码", trigger: ["blur", "input"] }],
}

async function submit() {
  if (loading.value) return
  try {
    await formRef.value?.validate()
  } catch {
    return
  }
  loading.value = true
  try {
    await auth.login({ username: form.username, password: form.password })
    message.success(`欢迎回来，${auth.user?.full_name || auth.user?.username || ""}`)
    const next = typeof route.query.next === "string" ? route.query.next : "/"
    router.replace(next)
  } catch (err) {
    if (err instanceof ApiError) {
      // toast already raised by interceptor; surface focused message anyway
      message.error(err.message)
    } else {
      message.error("登录失败")
    }
  } finally {
    loading.value = false
  }
}
</script>
