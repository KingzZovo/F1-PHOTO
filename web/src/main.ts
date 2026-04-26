import { createApp, h } from "vue"
import { createPinia } from "pinia"
import {
  create,
  NConfigProvider,
  NMessageProvider,
  NDialogProvider,
  NLoadingBarProvider,
  NNotificationProvider,
  zhCN,
  dateZhCN,
  useMessage,
} from "naive-ui"
import { router } from "@/router"
import { setApiToastSink } from "@/api/client"
import "@/style.css"

const naive = create()
const pinia = createPinia()

// Wire interceptor toasts to Naive's <n-message-provider>.
const MessageBridge = {
  setup() {
    const m = useMessage()
    setApiToastSink((msg, type = "error") => {
      switch (type) {
        case "success": m.success(msg); break
        case "warning": m.warning(msg); break
        case "info": m.info(msg); break
        default: m.error(msg); break
      }
    })
    return () => null
  },
}

const Root = {
  name: "Root",
  render() {
    return h(
      NConfigProvider,
      { locale: zhCN, dateLocale: dateZhCN },
      {
        default: () =>
          h(NLoadingBarProvider, null, {
            default: () =>
              h(NDialogProvider, null, {
                default: () =>
                  h(NNotificationProvider, null, {
                    default: () =>
                      h(NMessageProvider, null, {
                        default: () => [h(MessageBridge), h("router-view")],
                      }),
                  }),
              }),
          }),
      },
    )
  },
}

const app = createApp(Root)
app.use(pinia)
app.use(router)
app.use(naive)
app.mount("#app")
