import type { UsageViewerApi } from "./tauri-api"

declare global {
  interface Window {
    usageViewer: UsageViewerApi
  }
}

export {}
