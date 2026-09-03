import type { UsageViewerApi } from "../electron/preload"

declare global {
  interface Window {
    usageViewer: UsageViewerApi
  }
}

export {}
