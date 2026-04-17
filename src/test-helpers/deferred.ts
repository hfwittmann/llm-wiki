/**
 * A promise whose resolve/reject handles are exposed so tests can drive
 * completion timing manually. Essential for race/abort/concurrency tests
 * where we need to pause a long-running operation at a specific await point.
 */
export interface Deferred<T> {
  promise: Promise<T>
  resolve: (value: T) => void
  reject: (error: unknown) => void
  settled: boolean
}

export function createDeferred<T = void>(): Deferred<T> {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  const d: Deferred<T> = {
    promise,
    resolve: (v) => {
      if (d.settled) return
      d.settled = true
      resolve(v)
    },
    reject: (e) => {
      if (d.settled) return
      d.settled = true
      reject(e)
    },
    settled: false,
  }
  return d
}

/**
 * Yield to the microtask queue so pending .then / await continuations run.
 * Use after triggering an async operation you want to observe mid-flight.
 */
export async function flushMicrotasks(ticks: number = 5): Promise<void> {
  for (let i = 0; i < ticks; i++) {
    await Promise.resolve()
  }
}
