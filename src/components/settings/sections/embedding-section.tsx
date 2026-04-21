import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import type { SettingsDraft, DraftSetter } from "../settings-types"

interface Props {
  draft: SettingsDraft
  setDraft: DraftSetter
}

export function EmbeddingSection({ draft, setDraft }: Props) {
  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-xl font-semibold">向量嵌入</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          启用语义搜索。对任何 OpenAI 兼容的 /v1/embeddings endpoint 工作。
          本地模型(如 LM Studio、Ollama embedding)不需要 API Key。
        </p>
      </div>

      <div className="flex items-center justify-between rounded-md border p-3">
        <div>
          <div className="text-sm font-medium">启用向量搜索</div>
          <div className="text-xs text-muted-foreground">
            关掉后搜索只走 token 匹配。
          </div>
        </div>
        <button
          type="button"
          onClick={() => setDraft("embeddingEnabled", !draft.embeddingEnabled)}
          className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
            draft.embeddingEnabled ? "bg-primary" : "bg-muted"
          }`}
        >
          <span
            className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
              draft.embeddingEnabled ? "translate-x-4.5" : "translate-x-0.5"
            }`}
          />
        </button>
      </div>

      {draft.embeddingEnabled && (
        <>
          <div className="space-y-2">
            <Label>Endpoint</Label>
            <Input
              value={draft.embeddingEndpoint}
              onChange={(e) => setDraft("embeddingEndpoint", e.target.value)}
              placeholder="http://127.0.0.1:1234/v1/embeddings"
            />
          </div>

          <div className="space-y-2">
            <Label>API Key (optional)</Label>
            <Input
              type="password"
              value={draft.embeddingApiKey}
              onChange={(e) => setDraft("embeddingApiKey", e.target.value)}
              placeholder="本地模型留空即可"
            />
          </div>

          <div className="space-y-2">
            <Label>Model</Label>
            <Input
              value={draft.embeddingModel}
              onChange={(e) => setDraft("embeddingModel", e.target.value)}
              placeholder="e.g. text-embedding-qwen3-embedding-0.6b"
            />
          </div>
        </>
      )}
    </div>
  )
}
