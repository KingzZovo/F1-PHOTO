<template>
  <div class="p-6 space-y-4">
    <h1 class="text-2xl font-semibold">概览</h1>
    <p class="text-gray-600">欢迎使用 F1-photo。后续 turn 会在这里加上 projects/master/work_orders/recognition 各模块。</p>
    <n-card title="当前会话">
      <n-descriptions :column="1" label-placement="left" bordered>
        <n-descriptions-item label="用户">
          <span v-text="auth.user?.username || '—'" />
        </n-descriptions-item>
        <n-descriptions-item label="姓名">
          <span v-text="auth.user?.full_name || '—'" />
        </n-descriptions-item>
        <n-descriptions-item label="角色">
          <span v-text="auth.user?.role || '—'" />
        </n-descriptions-item>
        <n-descriptions-item label="Token 过期时间">
          <span v-text="auth.expiresAt || '—'" />
        </n-descriptions-item>
        <n-descriptions-item label="剩余有效期 (秒)">
          <span v-text="Math.round(auth.expiresInMs / 1000)" />
        </n-descriptions-item>
      </n-descriptions>
    </n-card>
  </div>
</template>

<script setup lang="ts">
import { NCard, NDescriptions, NDescriptionsItem } from "naive-ui"
import { useAuthStore } from "@/stores/auth"
const auth = useAuthStore()
</script>
