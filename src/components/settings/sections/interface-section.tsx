import { Label } from "@/components/ui/label"
import type { SettingsDraft, DraftSetter } from "../settings-types"

interface Props {
  draft: SettingsDraft
  setDraft: DraftSetter
}

const UI_LANGUAGES = [
  { value: "en", label: "English" },
  { value: "zh", label: "中文" },
]

export function InterfaceSection({ draft, setDraft }: Props) {
  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold">界面</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          应用 UI 的显示语言。切换后立即生效并持久化。
        </p>
      </div>

      <div className="space-y-2">
        <Label>UI 语言</Label>
        <div className="flex flex-wrap gap-2">
          {UI_LANGUAGES.map((l) => {
            const active = draft.uiLanguage === l.value
            return (
              <button
                key={l.value}
                type="button"
                onClick={() => setDraft("uiLanguage", l.value)}
                className={`rounded-md border px-3 py-1.5 text-sm transition-colors ${
                  active
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-border hover:bg-accent"
                }`}
              >
                {l.label}
              </button>
            )
          })}
        </div>
        <p className="text-xs text-muted-foreground">
          只影响按钮、标签这些 UI 文案,不影响 AI 输出语言(那个在"输出偏好"里单独设置)。
        </p>
      </div>
    </div>
  )
}
