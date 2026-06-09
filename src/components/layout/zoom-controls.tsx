import { Search, Plus, Minus, RotateCcw } from "lucide-react"
import { useZoomStore } from "@/stores/zoom-store"
import { cn } from "@/lib/utils"
import { useEffect, useRef } from "react"

const MIN_ZOOM = 0.5
const MAX_ZOOM = 3
const STEP = 0.05

export function ZoomControls() {
  const level = useZoomStore((s) => s.level)
  const open = useZoomStore((s) => s.open)
  const toggle = useZoomStore((s) => s.toggle)
  const setLevel = useZoomStore((s) => s.setLevel)
  const close = useZoomStore((s) => s.close)
  
  const containerRef = useRef<HTMLDivElement>(null)

  // Close when clicking outside
  useEffect(() => {
    if (!open) return

    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        close()
      }
    }

    document.addEventListener("mousedown", handleClickOutside)
    return () => document.removeEventListener("mousedown", handleClickOutside)
  }, [open, close])

  return (
    <div ref={containerRef} className="fixed right-4 top-4 z-50 flex flex-col items-end gap-2">
      {/* Floating magnifier button */}
      <button
        onClick={toggle}
        className={cn(
          "flex size-9 items-center justify-center rounded-full shadow-md transition-all",
          "bg-background text-foreground hover:bg-accent hover:text-accent-foreground",
          open && "ring-2 ring-ring",
        )}
        aria-label="Toggle zoom controls"
      >
        <Search className="size-4" />
      </button>

      {/* Slider popover */}
      {open && (
        <div
          className={cn(
            "flex flex-col gap-3 rounded-xl border bg-popover p-4 shadow-lg",
            "border-border",
            "min-w-[180px]",
          )}
        >
          {/* Header */}
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground">Zoom</span>
            <button
              onClick={() => setLevel(1)}
              className="flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground transition-colors"
              aria-label="Reset zoom to 100%"
            >
              <RotateCcw className="size-3.5" />
            </button>
          </div>

          {/* Slider row */}
          <div className="flex items-center gap-2">
            <button
              onClick={() => setLevel(Number((level - STEP).toFixed(2)))}
              disabled={level <= MIN_ZOOM}
              className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:opacity-30 disabled:pointer-events-none transition-colors"
              aria-label="Zoom out"
            >
              <Minus className="size-3.5" />
            </button>

            <input
              type="range"
              min={MIN_ZOOM * 100}
              max={MAX_ZOOM * 100}
              step={STEP * 100}
              value={Math.round(level * 100)}
              onChange={(e) => setLevel(Number(e.target.value) / 100)}
              className="h-2 w-full cursor-pointer appearance-none rounded-full bg-muted accent-primary
                [&::-webkit-slider-thumb]:size-4 [&::-webkit-slider-thumb]:appearance-none
                [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-primary
                [&::-webkit-slider-thumb]:shadow-sm [&::-webkit-slider-thumb]:transition-transform
                [&::-webkit-slider-thumb]:hover:scale-110
                [&::-moz-range-thumb]:size-4 [&::-moz-range-thumb]:rounded-full
                [&::-moz-range-thumb]:border-0 [&::-moz-range-thumb]:bg-primary
                [&::-moz-range-thumb]:shadow-sm"
            />

            <button
              onClick={() => setLevel(Number((level + STEP).toFixed(2)))}
              disabled={level >= MAX_ZOOM}
              className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground disabled:opacity-30 disabled:pointer-events-none transition-colors"
              aria-label="Zoom in"
            >
              <Plus className="size-3.5" />
            </button>
          </div>

          {/* Percentage label */}
          <div className="text-center text-xs font-semibold text-foreground">
            {Math.round(level * 100)}%
          </div>
        </div>
      )}
    </div>
  )
}
