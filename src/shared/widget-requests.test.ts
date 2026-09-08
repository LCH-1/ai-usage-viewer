import { describe, expect, it } from "vitest"

import { WidgetRequests } from "./widget-requests"

describe("widget request scope", () => {
  it("allows a fast account and a new listing while another account is still pending", () => {
    const requests = new WidgetRequests()
    const listing = requests.beginListing()!
    const slow = requests.beginAccount("slow")!
    const fast = requests.beginAccount("fast")!
    requests.finishListing(listing)
    requests.finishAccount("fast", fast)
    expect(requests.beginListing()).not.toBeNull()
    expect(requests.beginAccount("fast")).not.toBeNull()
    expect(requests.beginAccount("slow")).toBeNull()
    expect(requests.isCurrent(slow)).toBe(true)
  })

  it("prevents queued provider work and late UI results after the widget hides", () => {
    const requests = new WidgetRequests()
    const listing = requests.beginListing()!
    const pending = requests.beginAccount("one")!
    requests.pause()
    expect(requests.isCurrent(listing)).toBe(false)
    expect(requests.isCurrent(pending)).toBe(false)
    expect(requests.beginAccount("two")).toBeNull()
    expect(requests.beginListing()).toBeNull()
    expect(requests.refreshing).toBe(false)
  })

  it("lets reopening refresh immediately without an old completion releasing the new request", () => {
    const requests = new WidgetRequests()
    const previousListing = requests.beginListing()!
    const previousAccount = requests.beginAccount("one")!
    requests.pause()
    requests.resume()
    const listing = requests.beginListing()!
    const account = requests.beginAccount("one")!
    requests.finishListing(previousListing)
    requests.finishAccount("one", previousAccount)
    expect(requests.isCurrent(previousAccount)).toBe(false)
    expect(requests.isCurrent(account)).toBe(true)
    expect(requests.beginListing()).toBeNull()
    expect(requests.beginAccount("one")).toBeNull()
    requests.finishListing(listing)
    requests.finishAccount("one", account)
    expect(requests.refreshing).toBe(false)
  })
})
