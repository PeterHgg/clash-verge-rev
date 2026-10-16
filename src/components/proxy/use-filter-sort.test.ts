import { afterEach, expect, test, vi } from 'vitest'

import delayManager from '@/services/delay'
import type { ProxyGroupView } from '@/types/proxy-view'
import { compareByDelay } from '@/utils/delay'

import { filterSort } from './use-filter-sort'
import type { ResolvedMemberOccurrence } from './use-render-list'

const node = (memberIndex: number, delay: number, name = `${memberIndex}`) =>
  ({
    memberIndex,
    member: {
      kind: 'node',
      ref: { kind: 'node', name, recordId: `${memberIndex}` },
      node: {
        recordId: `${memberIndex}`,
        history: [{ delay }],
        extra: {
          'http://cp.cloudflare.com/generate_204': {
            history: [{ time: new Date(0).toISOString(), delay }],
          },
        },
        source: { kind: 'provider', providerName: `${memberIndex}` },
      },
    },
  }) as unknown as ResolvedMemberOccurrence

afterEach(() => vi.restoreAllMocks())

const enhancedGroup: Pick<ProxyGroupView, 'type' | 'enhancement'> = {
  type: 'LoadBalance',
  enhancement: {
    state: 'trial',
    reason: null,
    candidates: (
      [
        ['b-standby', 100, false, 'standby'],
        ['x-trial', 95, true, 'trial'],
        ['z-primary', 90, true, 'selected'],
        ['a-paused', 85, false, 'standby'],
        ['y-primary', 80, true, 'selected'],
        ['d-waiting-trial', 70, false, 'trial'],
        ['c-standby', 60, false, 'standby'],
      ] as const
    ).map(([name, score, selected, state]) => ({
      name,
      score,
      selected,
      state,
      reachable: true,
      freshHealth: true,
      proven: true,
      confidence: 1,
      meanRate: null,
      peakRate: null,
      latencyMs: null,
      cooldownRemainingSeconds: null,
      lastCheckedSecondsAgo: null,
    })),
  },
}

const enhancedMembers = () => [
  node(0, 1, 'b-standby'),
  node(1, 700, 'y-primary'),
  node(2, 4, 'a-paused'),
  node(3, 3, 'd-waiting-trial'),
  node(4, 600, 'x-trial'),
  node(5, 900, 'z-primary'),
  node(6, 2, 'c-standby'),
]

test.each([
  [0, [5, 1, 4, 0, 2, 3, 6]],
  [1, [5, 1, 4, 0, 6, 3, 2]],
  [2, [5, 1, 4, 2, 0, 6, 3]],
] as const)(
  'pins actual selected slots before trials with client sort %i',
  (sortType, order) => {
    vi.spyOn(Date, 'now').mockReturnValue(1000)
    const list = enhancedMembers()
    const before = list.slice()
    const candidates = enhancedGroup.enhancement!.candidates.slice()
    const result = filterSort(
      list,
      'enhanced',
      '',
      sortType,
      undefined,
      undefined,
      enhancedGroup,
    )
    expect(result).toEqual(order.map((index) => list[index]))
    expect(list).toEqual(before)
    expect(enhancedGroup.enhancement!.candidates).toEqual(candidates)
  },
)

test('keeps explicit filters without restoring hidden selected nodes', () => {
  const list = enhancedMembers()
  expect(
    filterSort(
      list,
      'enhanced-filter',
      'standby',
      2,
      undefined,
      undefined,
      enhancedGroup,
    ),
  ).toEqual([list[0], list[6]])
  expect(
    filterSort(
      list,
      'enhanced-filter',
      'primary',
      2,
      undefined,
      undefined,
      enhancedGroup,
    ),
  ).toEqual([list[5], list[1]])
  expect(
    filterSort(
      list,
      'enhanced-filter',
      'missing',
      2,
      undefined,
      undefined,
      enhancedGroup,
    ),
  ).toEqual([])
})

test('does not pin ordinary, disabled or unsupported groups', () => {
  vi.spyOn(Date, 'now').mockReturnValue(1000)
  const list = enhancedMembers()
  const groups: Pick<ProxyGroupView, 'type' | 'enhancement'>[] = [
    { type: 'LoadBalance', enhancement: null },
    {
      type: 'LoadBalance',
      enhancement: { ...enhancedGroup.enhancement!, state: 'unsupported' },
    },
    { type: 'Selector', enhancement: enhancedGroup.enhancement },
  ]
  for (const sortType of [0, 1, 2] as const) {
    const expected = filterSort(list, 'ordinary', '', sortType)
    for (const group of groups) {
      expect(
        filterSort(list, 'ordinary', '', sortType, undefined, undefined, group),
      ).toEqual(expected)
    }
  }
})

test('matches the previous comparator for cached, fallback and sentinel delays', () => {
  vi.spyOn(Date, 'now').mockReturnValue(1000)
  const values = [30, 0, -2, -1, 1e6, 10000, 30, NaN, Infinity]
  const list = values.map((delay, i) => node(i, delay))
  list.push(node(10, 50, 'same'), node(11, 5, 'same'), list[0])
  list.push({
    memberIndex: 12,
    member: {
      kind: 'unresolved',
      ref: { kind: 'unresolved', name: 'missing', reason: 'missing' },
    },
  })
  for (const cached of [false, true]) {
    const group = `sort-${cached}`
    if (cached) {
      values.forEach((delay, i) =>
        delayManager.setDelay(list[i].member, group, delay),
      )
    }
    for (const timeout of [10000, 20, 0, NaN]) {
      const expected = list
        .slice()
        .sort((a, b) =>
          compareByDelay(
            delayManager.getDelayFix(a.member, group),
            delayManager.getDelayFix(b.member, group),
            timeout > 0 ? timeout : 10000,
          ),
        )
      const before = list.slice()
      const result = filterSort(list, group, '', 1, timeout)
      expect(result).toEqual(expected)
      result.forEach((item, i) => expect(item).toBe(expected[i]))
      expect(list).toEqual(before)
    }
  }
})

test('reads each occurrence once and observes cache updates and expiry on the next call', () => {
  const clock = vi.spyOn(Date, 'now').mockReturnValue(1000)
  const list = [
    node(0, 50),
    node(1, 15, 'same'),
    node(2, 5, 'same'),
    node(3, 25),
  ]
  const cachedOrder = [list[0], list[2], list[1], list[3]]
  delayManager.setDelay(list[0].member, 'expiry', 1)
  const get = vi.spyOn(delayManager, 'getDelayFix')
  expect(filterSort(list, 'expiry', '', 1)).toEqual(cachedOrder)
  expect(get).toHaveBeenCalledTimes(list.length)
  list.forEach((item, i) => expect(get.mock.calls[i][0]).toBe(item.member))
  clock.mockReturnValue(1000 + 30 * 60 * 1000)
  expect(filterSort(list, 'expiry', '', 1)).toEqual(cachedOrder)
  clock.mockReturnValue(1001 + 30 * 60 * 1000)
  expect(filterSort(list, 'expiry', '', 1)).toEqual([
    list[2],
    list[1],
    list[3],
    list[0],
  ])
  delayManager.setDelay(list[0].member, 'expiry', 2)
  expect(filterSort(list, 'expiry', '', 1)).toEqual(cachedOrder)
  get.mockClear()
  filterSort([], 'expiry', '', 1)
  filterSort([list[0]], 'expiry', '', 1)
  expect(filterSort(list, 'expiry', '', 0)).toBe(list)
  filterSort(list, 'expiry', '', 2)
  expect(get).not.toHaveBeenCalled()
})
