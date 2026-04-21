import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import type { SettingsDraft, DraftSetter } from "../settings-types"

interface Props {
  draft: SettingsDraft
  setDraft: DraftSetter
}

export function WebSearchSection({ draft, setDraft }: Props) {
  const options = [
    { value: "none" as const, label: "Disabled" },
    { value: "tavily" as const, label: "Tavily" },
  ]

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold">网页搜索 (Deep Research)</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          允许 AI 在发现知识缺口时自动调用网页搜索补充资料。目前接入 Tavily。
        </p>
      </div>

      <div className="space-y-2">
        <Label>搜索 Provider</Label>
        <div className="flex flex-wrap gap-2">
          {options.map((p) => {
            const active = draft.searchProvider === p.value
            return (
              <button
                key={p.value}
                type="button"
                onClick={() => setDraft("searchProvider", p.value)}
                className={`rounded-md border px-3 py-1.5 text-sm transition-colors ${
                  active
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-border hover:bg-accent"
                }`}
              >
                {p.label}
              </button>
            )
          })}
        </div>
      </div>

      {draft.searchProvider !== "none" && (
        <div className="space-y-2">
          <Label>API Key</Label>
          <Input
            type="password"
            value={draft.searchApiKey}
            onChange={(e) => setDraft("searchApiKey", e.target.value)}
            placeholder="Enter your Tavily API key (tavily.com)"
          />
        </div>
      )}
    </div>
  )
}
