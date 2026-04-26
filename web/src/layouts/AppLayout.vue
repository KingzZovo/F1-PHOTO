<template>
  <n-layout class="h-screen" has-sider>
    <n-layout-sider
      bordered
      collapse-mode="width"
      :collapsed-width="56"
      :width="220"
      show-trigger
      :native-scrollbar="false"
    >
      <div class="px-4 py-4 border-b border-gray-200">
        <span class="font-semibold text-base">F1-photo</span>
      </div>
      <n-menu :options="menu" :value="current" @update:value="go" />
    </n-layout-sider>
    <n-layout>
      <n-layout-header bordered class="flex items-center justify-between px-6 h-14">
        <span class="text-gray-600" v-text="pageTitle" />
        <n-space align="center">
          <span class="text-xs text-gray-500" v-text="`剩余 ${Math.round(auth.expiresInMs / 1000)}s`" />
          <n-dropdown :options="userMenu" trigger="click" @select="onUserMenu">
            <n-button text>
              <template #icon>👤</template>
              <span v-text="auth.user?.full_name || auth.user?.username || '用户'" />
            </n-button>
          </n-dropdown>
        </n-space>
      </n-layout-header>
      <n-layout-content :native-scrollbar="false">
        <router-view />
      </n-layout-content>
    </n-layout>
  </n-layout>
</template>

<script setup lang="ts">
import { computed } from "vue"
import { useRoute, useRouter } from "vue-router"
import {
  NLayout,
  NLayoutHeader,
  NLayoutSider,
  NLayoutContent,
  NMenu,
  NDropdown,
  NButton,
  NSpace,
  type MenuOption,
  type DropdownOption,
  useDialog,
} from "naive-ui"
import { useAuthStore } from "@/stores/auth"

const auth = useAuthStore()
const route = useRoute()
const router = useRouter()
const dialog = useDialog()

const menu: MenuOption[] = [
  { label: "概览", key: "home" },
  // turn 13+ will append: projects, persons/tools/devices, work orders, recognition
]

const current = computed(() => (typeof route.name === "string" ? route.name : "home"))
const pageTitle = computed(() => {
  const m = menu.find((x) => x.key === current.value)
  return m?.label?.toString() || ""
})

function go(key: string) {
  router.push({ name: key }).catch(() => {})
}

const userMenu: DropdownOption[] = [
  { label: "退出登录", key: "logout" },
]

async function onUserMenu(key: string) {
  if (key === "logout") {
    dialog.warning({
      title: "退出登录",
      content: "确认退出当前账号吗？",
      positiveText: "退出",
      negativeText: "取消",
      onPositiveClick: async () => {
        await auth.logout()
        router.replace({ name: "login" })
      },
    })
  }
}
</script>
