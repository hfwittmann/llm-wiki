import { loadTheme } from "@/lib/project-store"

export type AppTheme = "light" | "dark" | "system"

let activeTheme: AppTheme = "system"
let mediaQuery: MediaQueryList | null = null
let mediaListenerInstalled = false

function systemPrefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches
}

export function applyTheme(theme: AppTheme): void {
  activeTheme = theme
  const root = document.documentElement
  const resolved = theme === "system"
    ? systemPrefersDark()
      ? "dark"
      : "light"
    : theme

  root.classList.remove("light", "dark")
  root.classList.add(resolved)
  root.dataset.theme = theme
}

export function watchSystemTheme(): void {
  if (mediaListenerInstalled) return
  mediaQuery = window.matchMedia("(prefers-color-scheme: dark)")
  mediaQuery.addEventListener("change", () => {
    if (activeTheme === "system") applyTheme("system")
  })
  mediaListenerInstalled = true
}

export async function loadAndApplyTheme(): Promise<AppTheme> {
  try {
    const savedTheme = await loadTheme()
    const theme = savedTheme ?? "system"
    applyTheme(theme)
    return theme
  } catch {
    applyTheme("system")
    return "system"
  }
}
