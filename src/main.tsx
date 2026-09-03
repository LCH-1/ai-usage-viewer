import React from "react"
import ReactDOM from "react-dom/client"

import { App } from "./App"
import { installDemoApi } from "./shared/demo-api"
import "./styles.css"

if (!window.usageViewer && new URLSearchParams(window.location.search).has("demo")) installDemoApi()

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
