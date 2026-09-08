interface RequestTicket {
  generation: number
}

export class WidgetRequests {
  private generation = 0
  private active = true
  private listing: RequestTicket | null = null
  private accounts = new Map<string, RequestTicket>()

  isCurrent(ticket: RequestTicket): boolean {
    return this.active && ticket.generation === this.generation
  }

  beginListing(): RequestTicket | null {
    if (!this.active || this.listing) return null
    const ticket = { generation: this.generation }
    this.listing = ticket
    return ticket
  }

  finishListing(ticket: RequestTicket): void {
    if (this.listing === ticket) this.listing = null
  }

  hasAccount(accountId: string): boolean {
    return this.accounts.has(accountId)
  }

  beginAccount(accountId: string): RequestTicket | null {
    if (!this.active || this.accounts.has(accountId)) return null
    const ticket = { generation: this.generation }
    this.accounts.set(accountId, ticket)
    return ticket
  }

  finishAccount(accountId: string, ticket: RequestTicket): void {
    if (this.accounts.get(accountId) === ticket) this.accounts.delete(accountId)
  }

  get refreshing(): boolean {
    return this.accounts.size > 0
  }

  pause(): void {
    this.active = false
    this.generation += 1
    this.listing = null
    this.accounts.clear()
  }

  resume(): void {
    this.active = true
  }
}
