import { create } from "zustand"

export interface ZoomState {
  /** Current zoom level as a decimal (1 = 100%) */
  level: number
  /** Whether the zoom slider popover is open */
  open: boolean
  setLevel: (level: number) => void
  toggle: () => void
  close: () => void
}

/**
 * Clamp the zoom level between 0.5 (50%) and 3 (300%).
 */
function clamp(v: number): number {
  return Math.min(3, Math.max(0.5, v))
}

export const useZoomStore = create<ZoomState>((set) => ({
  level: 1,
  open: false,
  setLevel: (level: number) => set({ level: clamp(level) }),
  toggle: () => set((s) => ({ open: !s.open })),
  close: () => set({ open: false }),
}))
