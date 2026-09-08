import { describe, expect, it } from "vitest"

import { AccountRequestCoordinator } from "./account-requests"

function deferred() {
  let finish = () => undefined as void
  const promise = new Promise<void>((resolve) => { finish = resolve })
  return { promise, finish }
}

describe("account request coordination", () => {
  it("shares a slow request when manual and automatic refresh overlap", async () => {
    const coordinator = new AccountRequestCoordinator()
    const pending = deferred()
    let calls = 0
    const operation = async () => { calls += 1; await pending.promise }
    const first = coordinator.refresh("one", operation)
    const second = coordinator.refresh("one", operation)
    expect(second).toBe(first)
    await Promise.resolve()
    expect(calls).toBe(1)
    pending.finish()
    await first
    await coordinator.refresh("one", operation)
    expect(calls).toBe(2)
  })

  it("keeps polling from interrupting login and ignores the previous session response", async () => {
    const coordinator = new AccountRequestCoordinator()
    const refresh = deferred()
    const login = deferred()
    let previousResponseApplied = false
    let polled = false
    const first = coordinator.refresh("one", async (isCurrent) => {
      await refresh.promise
      previousResponseApplied = isCurrent()
    })
    await Promise.resolve()
    const authenticating = coordinator.authenticate("one", async () => { await login.promise })
    expect(coordinator.authenticate("one", async () => { throw new Error("duplicate login") })).toBe(authenticating)
    await coordinator.refresh("one", async () => { polled = true })
    refresh.finish()
    await first
    expect(previousResponseApplied).toBe(false)
    expect(polled).toBe(false)
    expect(coordinator.isAuthenticating("one")).toBe(true)
    login.finish()
    await authenticating
    expect(coordinator.isAuthenticating("one")).toBe(false)
  })

  it("invalidates login completion while deletion or cancellation is pending", async () => {
    const coordinator = new AccountRequestCoordinator()
    const login = deferred()
    let applied = false
    let refreshed = false
    const authenticating = coordinator.authenticate("one", async (isCurrent) => {
      await login.promise
      applied = isCurrent()
    })
    await Promise.resolve()
    coordinator.pause("one")
    await coordinator.refresh("one", async () => { refreshed = true })
    login.finish()
    await authenticating
    expect(applied).toBe(false)
    expect(refreshed).toBe(false)
    coordinator.resume("one")
    await coordinator.refresh("one", async () => { refreshed = true })
    expect(refreshed).toBe(true)
  })

  it("lets other accounts proceed and releases a failed request for retry", async () => {
    const coordinator = new AccountRequestCoordinator()
    const login = deferred()
    const authenticating = coordinator.authenticate("one", async () => { await login.promise })
    let otherAccountUpdated = false
    await coordinator.refresh("two", async () => { otherAccountUpdated = true })
    expect(otherAccountUpdated).toBe(true)
    await expect(coordinator.refresh("two", async () => { throw new Error("offline") })).rejects.toThrow("offline")
    await coordinator.refresh("two", async () => undefined)
    login.finish()
    await authenticating
  })
})
