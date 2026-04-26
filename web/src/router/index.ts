import { createRouter, createWebHistory, type RouteRecordRaw } from "vue-router"
import { useAuthStore } from "@/stores/auth"

const routes: RouteRecordRaw[] = [
  {
    path: "/login",
    name: "login",
    component: () => import("@/views/LoginView.vue"),
    meta: { public: true },
  },
  {
    path: "/",
    component: () => import("@/layouts/AppLayout.vue"),
    children: [
      {
        path: "",
        name: "home",
        component: () => import("@/views/HomeView.vue"),
      },
      {
        path: "projects",
        name: "projects",
        component: () => import("@/views/ProjectsListView.vue"),
      },
      {
        path: "projects/:id",
        name: "project-detail",
        component: () => import("@/views/ProjectDetailView.vue"),
      },
      {
        path: "persons",
        name: "persons",
        component: () => import("@/views/PersonsView.vue"),
      },
      {
        path: "tools",
        name: "tools",
        component: () => import("@/views/ToolsView.vue"),
      },
      {
        path: "devices",
        name: "devices",
        component: () => import("@/views/DevicesView.vue"),
      },
      {
        path: "work-orders",
        name: "work-orders",
        component: () => import("@/views/WorkOrdersView.vue"),
      },
    ],
  },
  { path: "/:pathMatch(.*)*", redirect: "/" },
]

export const router = createRouter({
  history: createWebHistory("/"),
  routes,
})

router.beforeEach(async (to) => {
  const auth = useAuthStore()
  if (!auth.initialized && !auth.initializing) await auth.bootstrap()
  if (to.meta.public) {
    if (auth.isAuthenticated && to.name === "login") return { name: "home" }
    return true
  }
  if (!auth.isAuthenticated) {
    return { name: "login", query: { next: to.fullPath } }
  }
  return true
})
