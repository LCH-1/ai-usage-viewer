import React from "react"
import ReactDOM from "react-dom/client"

import { App } from "./App"
import { UsageWidget } from "./UsageWidget"
import { installDemoApi } from "./shared/demo-api"
import { usageViewerApi } from "./tauri-api"
import "./styles.css"

if (new URLSearchParams(window.location.search).has("demo")) installDemoApi()
else window.usageViewer = usageViewerApi

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {new URLSearchParams(window.location.search).has("widget") ? <UsageWidget /> : <App />}
  </React.StrictMode>,
)
