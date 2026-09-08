type Operation = (isCurrent: () => boolean) => Promise<void>

interface AccountRequests {
  revision: number
  paused: boolean
  refresh?: Promise<void>
  authentication?: Promise<void>
}

export class AccountRequestCoordinator {
  private accounts = new Map<string, AccountRequests>()

  private state(accountId: string): AccountRequests {
    let state = this.accounts.get(accountId)
    if (!state) {
      state = { revision: 0, paused: false }
      this.accounts.set(accountId, state)
    }
    return state
  }

  private run(accountId: string, kind: "refresh" | "authentication", operation: Operation): Promise<void> {
    const state = this.state(accountId)
    if (state.paused || (kind === "refresh" && state.authentication)) return Promise.resolve()
    const pending = state[kind]
    if (pending) return pending
    if (kind === "authentication") this.invalidate(accountId)
    const revision = state.revision
    const isCurrent = () => this.accounts.get(accountId) === state && state.revision === revision
    const request = Promise.resolve()
      .then(() => { if (isCurrent()) return operation(isCurrent) })
      .finally(() => { if (isCurrent() && state[kind] === request) state[kind] = undefined })
    state[kind] = request
    return request
  }

  refresh(accountId: string, operation: Operation): Promise<void> {
    return this.run(accountId, "refresh", operation)
  }

  authenticate(accountId: string, operation: Operation): Promise<void> {
    return this.run(accountId, "authentication", operation)
  }

  isAuthenticating(accountId: string): boolean {
    return Boolean(this.state(accountId).authentication)
  }

  invalidate(accountId: string): void {
    const state = this.state(accountId)
    state.revision += 1
    state.refresh = undefined
    state.authentication = undefined
  }

  pause(accountId: string): void {
    this.invalidate(accountId)
    this.state(accountId).paused = true
  }

  resume(accountId: string): void {
    this.state(accountId).paused = false
  }
}
