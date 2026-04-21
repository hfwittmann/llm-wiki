import { useEffect, useState } from "react"
import { clipServerStatus } from "@/commands/fs"

export function AboutSection() {
  const [clipStatus, setClipStatus] = useState<string>("...")

  useEffect(() => {
    let alive = true
    clipServerStatus()
      .then((s) => {
        if (alive) setClipStatus(s)
      })
      .catch(() => {
        if (alive) setClipStatus("unknown")
      })
    return () => {
      alive = false
    }
  }, [])

  const rows: Array<{ label: string; value: string; mono?: boolean }> = [
    { label: "版本", value: `v${__APP_VERSION__}`, mono: true },
    { label: "Clip Server", value: `${clipStatus}  @  127.0.0.1:19827`, mono: true },
  ]

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold">关于</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          构建信息和运行时状态。
        </p>
      </div>

      <div className="rounded-md border divide-y">
        {rows.map((r) => (
          <div key={r.label} className="flex items-center justify-between px-4 py-2.5">
            <span className="text-sm text-muted-foreground">{r.label}</span>
            <span className={`text-sm ${r.mono ? "font-mono" : ""}`}>{r.value}</span>
          </div>
        ))}
      </div>

      <div className="rounded-md border p-4 text-sm">
        <div className="font-medium">LLM Wiki</div>
        <p className="mt-1 text-xs text-muted-foreground">
          An LLM-driven personal knowledge base. Source:
          {" "}
          <a
            className="underline underline-offset-2 hover:text-primary"
            href="https://github.com/nashsu/llm_wiki"
            target="_blank"
            rel="noreferrer"
          >
            github.com/nashsu/llm_wiki
          </a>
        </p>
      </div>
    </div>
  )
}
