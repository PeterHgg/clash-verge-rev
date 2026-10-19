import { createInstance } from 'i18next'
import { beforeAll, beforeEach, expect, it, vi } from 'vitest'

import enSettings from '@/locales/en/settings.json'
import zhSettings from '@/locales/zh/settings.json'
import { appStoreReducer, initialAppStoreState } from '@/store/app-state'

import type {
  CloudflareCapability_Serialize,
  CloudflareDiagnostic,
  CloudflareDnsBinding,
  CloudflareDnsInventory_Serialize,
  CloudflareSnapshot,
} from './bindings'
import {
  CLOUDFLARE_PREFERRED_DOMAINS,
  CLOUDFLARE_TEST_URL,
  applyCloudflareDnsBindings,
  eligibleCloudflareDomains,
  formatCloudflareBytes,
  formatCloudflareDiagnostic,
  formatCloudflareMbps,
  formatCloudflareRounds,
  formatCloudflareScore,
  hasCloudflareDownload,
  rankCloudflareResults,
  summarizeCloudflareSamples,
  supportsCloudflareDomainTest,
  upsertCloudflareBinding,
  type CloudflareResult,
} from './cloudflare-speed-test'
import { setCloudflareDnsBindings } from './cmds'
import { mutate } from './mutate'
import { errorDetail, showNotice } from './notice-service'
import { revalidateQueries } from './query-client'

vi.mock('./cmds', () => ({ setCloudflareDnsBindings: vi.fn() }))
vi.mock('./query-client', () => ({
  revalidateQueries: vi.fn().mockResolvedValue([]),
}))

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(setCloudflareDnsBindings).mockReset()
})

const i18n = createInstance()
const translate = i18n.t as Parameters<typeof formatCloudflareDiagnostic>[0]
beforeAll(async () => {
  await i18n.init({
    lng: 'zh',
    fallbackLng: 'en',
    resources: {
      zh: { translation: { settings: zhSettings } },
      en: { translation: { settings: enSettings } },
    },
  })
})

const result = (
  ip: string,
  state: CloudflareResult['state'],
  score?: number,
): CloudflareResult => ({
  ip,
  state,
  score,
  sources: [],
  bytesRead: 1024,
  durationMs: state === 'complete' || state === 'short' ? 10 : 0,
  failures: 0,
  attempts: 1,
  attemptedRounds: state === 'complete' || state === 'short' ? 2 : 0,
  successfulRounds: state === 'complete' ? 2 : state === 'short' ? 1 : 0,
  ...(state === 'complete' || state === 'short'
    ? { meanRate: 1000, peakRate: 1200 }
    : {}),
})

const snapshot = (
  runId: string,
  revision: number,
  startedAt: string,
): CloudflareSnapshot => ({
  schemaVersion: 1,
  capability: 'verge-cloudflare1',
  method: 'verge-cfst-transfer-v3',
  latencyMethod: 'tcp-connect',
  runId,
  revision,
  startedAt,
  state: 'running',
  stage: 'screening',
  interfaceName: 'eth0',
  dnsMode: 'system',
  url: 'https://speed.cloudflare.com/',
  bytesRead: 0,
  budgetBytes: 268435456,
  completedTargets: 0,
  totalTargets: 0,
  results: [],
  sources: [],
})

it('ranks a fast valid round above a slower complete measurement without mutating results', () => {
  const input = [
    result('1.1.1.1', 'screened', 1),
    { ...result('1.0.0.1', 'short', 0.9), meanRate: 4000 },
    result('104.16.0.1', 'complete', 0.2),
  ]
  expect(rankCloudflareResults(input).map((item) => item.state)).toEqual([
    'short',
    'complete',
    'screened',
  ])
  expect(input[0].state).toBe('screened')
  expect(hasCloudflareDownload(input[0])).toBe(false)
  expect(formatCloudflareMbps(undefined)).toBe('—')
  expect(formatCloudflareMbps(1_000_000)).toBe('8.00')
  expect(formatCloudflareBytes(268435456)).toBe('256.00 MiB')
  expect(CLOUDFLARE_PREFERRED_DOMAINS).toHaveLength(3)
})

it('requires partial-round-aware domain transfers and TCP-connect timing before enabling a test', () => {
  const capability: CloudflareCapability_Serialize = {
    schemaVersion: 1,
    capability: 'verge-cloudflare1',
    supported: true,
    preferredDomains: [...CLOUDFLARE_PREFERRED_DOMAINS],
    budgetBytes: 268435456,
    dnsAliases: true,
    fixedAddressFamily: true,
    domainOnly: true,
    engineVersion: 'cf-speedtest3',
    method: 'verge-cfst-transfer-v3',
    latencyMethod: 'tcp-connect',
  }
  expect(supportsCloudflareDomainTest(capability)).toBe(true)
  expect(supportsCloudflareDomainTest(null)).toBe(false)
  for (const old of [
    { domainOnly: false },
    { engineVersion: 'cf-speedtest1' },
    { engineVersion: 'cf-speedtest2' },
    { method: 'verge-cfst-transfer-v1' },
    { method: 'verge-cfst-transfer-v2' },
    { latencyMethod: '' },
    { latencyMethod: 'rtt' },
    { supported: false },
  ]) {
    expect(supportsCloudflareDomainTest({ ...capability, ...old })).toBe(false)
  }
  expect(CLOUDFLARE_TEST_URL).toBe(
    'https://speed.cloudflare.com/__down?bytes=99999999',
  )
  expect(capability.budgetBytes).toBe(256 * 1024 * 1024)
})

it('retains shared-IP membership and counts unmeasured samples without inventing domain metrics', () => {
  const [first, second, third] = CLOUDFLARE_PREFERRED_DOMAINS
  const shared = {
    ...result('1.1.1.1', 'short', 0.9),
    sources: [first, second],
  }
  const failed = { ...result('1.0.0.1', 'failed'), sources: [first] }
  const waiting = { ...result('104.16.0.1', 'untested'), sources: [first] }
  const samples = [shared, failed, waiting]
  expect(summarizeCloudflareSamples(samples, first)).toEqual({
    samples,
    success: 1,
    failed: 1,
    untested: 1,
  })
  expect(summarizeCloudflareSamples(samples, second)).toEqual({
    samples: [shared],
    success: 1,
    failed: 0,
    untested: 0,
  })
  expect(summarizeCloudflareSamples(samples, third)).toEqual({
    samples: [],
    success: 0,
    failed: 0,
    untested: 0,
  })
})

it('keeps a normal short EOF measurable but excludes empty or unfinished samples', () => {
  const short = {
    ...result('1.1.1.1', 'short', 1),
    finishReason: 'eof' as const,
  }
  const empty = { ...result('1.0.0.1', 'short', 1), meanRate: undefined }
  const unfinished = { ...result('104.16.0.1', 'untested', 1), meanRate: 2000 }
  expect(hasCloudflareDownload(short)).toBe(true)
  expect(hasCloudflareDownload(empty)).toBe(false)
  expect(hasCloudflareDownload(unfinished)).toBe(false)
  expect(hasCloudflareDownload({ ...short, durationMs: 0 })).toBe(false)
  expect(hasCloudflareDownload({ ...short, meanRate: 0 })).toBe(false)
  expect(formatCloudflareBytes(empty.bytesRead)).toBe('1.0 KiB')
  expect(formatCloudflareMbps(empty.meanRate)).toBe('—')
  expect(
    rankCloudflareResults([short, result('104.16.0.2', 'complete', 0.1)])[0]
      .state,
  ).toBe('short')
})

it('uses completeness only to break equal scores, then orders IPs deterministically', () => {
  const input = [
    result('1.1.1.1', 'short', 0.5),
    result('104.16.0.2', 'complete', 0.5),
    result('104.16.0.1', 'complete', 0.5),
  ]
  expect(rankCloudflareResults(input).map((item) => item.ip)).toEqual([
    '104.16.0.1',
    '104.16.0.2',
    '1.1.1.1',
  ])
})

it('shows real zero scores and unknown invalid scores without blocking a valid round', () => {
  const measured = result('1.1.1.1', 'short', 0)
  const unknown = { ...measured, ip: '1.0.0.1', score: undefined }
  expect(formatCloudflareScore(measured)).toBe('0.0')
  expect(formatCloudflareScore(result('104.16.0.1', 'complete', 1))).toBe(
    '100.0',
  )
  for (const score of [undefined, NaN, Infinity, -0.1, 1.1]) {
    const sample = { ...measured, score }
    expect(formatCloudflareScore(sample)).toBe('—')
    expect(hasCloudflareDownload(sample)).toBe(true)
  }
  expect(formatCloudflareScore(result('104.16.0.2', 'failed', 1))).toBe('—')
  expect(rankCloudflareResults([unknown, measured])[0]).toBe(measured)
  expect(
    rankCloudflareResults([result('104.16.0.2', 'screened', 1), unknown])[0],
  ).toBe(unknown)
})

it('keeps one valid round selectable and counted despite a failed round diagnostic', () => {
  const domain = CLOUDFLARE_PREFERRED_DOMAINS[0]
  const partial = {
    ...result('1.1.1.1', 'short', 0.9),
    sources: [domain],
    diagnostic: { stage: 'headers', code: 'setup_timeout' } as const,
  }
  expect(hasCloudflareDownload(partial)).toBe(true)
  expect(formatCloudflareRounds(translate, partial)).toBe(
    '有效 1 轮 · 已测 2 次',
  )
  expect(formatCloudflareDiagnostic(translate, partial.diagnostic)).toContain(
    zhSettings.modals.cloudflare.diagnostics.reasons.setupTimeout,
  )
  expect(summarizeCloudflareSamples([partial], domain)).toEqual({
    samples: [partial],
    success: 1,
    failed: 0,
    untested: 0,
  })
  expect(
    formatCloudflareRounds(translate, {
      successfulRounds: 0,
      attemptedRounds: 3,
    }),
  ).toBe('有效 0 轮 · 已测 3 次')
  expect(
    formatCloudflareRounds(i18n.getFixedT('en') as typeof translate, partial),
  ).toBe('Valid rounds: 1 · Attempts: 2')
})

it('does not treat failed-byte observations or invalid round metrics as valid downloads', () => {
  const partial = result('1.1.1.1', 'short', 0.5)
  for (const invalid of [
    { successfulRounds: 0 },
    { successfulRounds: 2, attemptedRounds: 1 },
    { successfulRounds: 1.5 },
    { attemptedRounds: 4 },
    { meanRate: 0 },
    { meanRate: NaN },
    { meanRate: Infinity },
    { durationMs: 0 },
    { durationMs: Infinity },
    { bytesRead: 0 },
    { state: 'complete' as const, successfulRounds: 1 },
  ]) {
    expect(hasCloudflareDownload({ ...partial, ...invalid })).toBe(false)
  }
  expect(hasCloudflareDownload(result('1.0.0.1', 'failed'))).toBe(false)
})

it.each<[CloudflareDiagnostic, string]>([
  [{ stage: 'http', code: 'http_status', httpStatus: 403 }, '拒绝此地址'],
  [{ stage: 'http', code: 'http_status', httpStatus: 503 }, '服务器错误'],
  [{ stage: 'http', code: 'http_status', httpStatus: 302 }, '未返回 HTTP 200'],
  [
    {
      stage: 'http',
      code: 'rate_limited',
      httpStatus: 429,
      retryAfterSeconds: 15,
    },
    '至少等待 15 秒',
  ],
  [
    { stage: 'content', code: 'unsupported_content', contentType: 'html' },
    '内容类型：HTML',
  ],
  [{ stage: 'tls', code: 'tls_failed' }, '证书校验失败'],
  [{ stage: 'body', code: 'no_measured_data' }, '只显示实际字节'],
  [{ stage: 'body', code: 'body_read_failed' }, '不属于短文件的正常结束'],
])(
  'localizes structured diagnostic %j instead of its raw English error',
  (diagnostic, expected) => {
    const formatted = formatCloudflareDiagnostic(
      translate,
      diagnostic,
      'legacy raw English error',
    )
    expect(formatted).toContain(expected)
    expect(formatted).not.toContain('legacy raw English')
    if (diagnostic.httpStatus != null) {
      expect(formatted).toContain(`HTTP ${diagnostic.httpStatus}`)
    }
  },
)

it('falls back to legacy errors and formats command failures without object coercion', () => {
  expect(formatCloudflareDiagnostic(translate, null, 'legacy error')).toBe(
    'legacy error',
  )
  expect(formatCloudflareDiagnostic(translate, undefined)).toBeNull()
  const failure = { code: 'OPERATION_FAILED', detail: 'Actionable detail' }
  expect(errorDetail(failure)).toBe('Actionable detail')
  expect(errorDetail(new Error('native failure'))).toBe('native failure')
  expect(errorDetail({ message: 'lookup failed' })).toBe('lookup failed')
  expect(errorDetail('legacy command error')).toBe('legacy command error')
})

const binding = (domain = 'node.example'): CloudflareDnsBinding => ({
  domain,
  target: { kind: 'preferred_domain', domain: CLOUDFLARE_PREFERRED_DOMAINS[0] },
})
const confirmation = {
  status: 'confirmation_required' as const,
  confirmation: 'captured-token',
  conflicts: [],
  warnings: [],
  affectedDomains: ['node.example'],
  enableUseHosts: true,
}

it('applies one captured binding intent with one confirmation and no intervening revalidation', async () => {
  let release!: () => void
  const pending = new Promise<void>((resolve) => {
    release = resolve
  })
  const context = { profileUid: 'profile-a', fingerprint: 'fingerprint-a' }
  const bindings = [binding()]
  const expected = structuredClone(bindings)
  vi.mocked(setCloudflareDnsBindings)
    .mockImplementationOnce(async () => {
      await pending
      return confirmation
    })
    .mockResolvedValueOnce({ status: 'applied', saved: true })
  const success = vi.spyOn(showNotice, 'success')
  const error = vi.spyOn(showNotice, 'error')
  const applied = applyCloudflareDnsBindings(context, bindings)
  expect(revalidateQueries).not.toHaveBeenCalled()
  const otherWrite = vi.fn().mockResolvedValue(undefined)
  const queued = mutate(otherWrite, {
    id: 'cloudflare-dns-bindings',
    errorNotice: false,
  })
  expect(otherWrite).not.toHaveBeenCalled()
  context.profileUid = 'profile-b'
  context.fingerprint = 'fingerprint-b'
  bindings[0].target = { kind: 'fixed_ip', ip: '1.1.1.1', runId: 'new-run' }
  bindings.push(binding('another.example'))
  release()
  expect(await applied).toEqual({ status: 'applied', saved: true })
  await queued
  expect(setCloudflareDnsBindings).toHaveBeenCalledTimes(2)
  expect(setCloudflareDnsBindings).toHaveBeenNthCalledWith(
    1,
    'profile-a',
    'fingerprint-a',
    expected,
    true,
  )
  expect(setCloudflareDnsBindings).toHaveBeenNthCalledWith(
    2,
    'profile-a',
    'fingerprint-a',
    expected,
    true,
    'captured-token',
  )
  expect(vi.mocked(setCloudflareDnsBindings).mock.calls[0][2]).toBe(
    vi.mocked(setCloudflareDnsBindings).mock.calls[1][2],
  )
  expect(revalidateQueries).toHaveBeenCalledOnce()
  expect(
    vi.mocked(revalidateQueries).mock.invocationCallOrder[0],
  ).toBeGreaterThan(
    vi.mocked(setCloudflareDnsBindings).mock.invocationCallOrder[1],
  )
  expect(otherWrite).toHaveBeenCalledOnce()
  expect(success).not.toHaveBeenCalled()
  expect(error).not.toHaveBeenCalled()
  success.mockRestore()
  error.mockRestore()
})

it('replaces a saved source once and persists the last deletion with enabled false', async () => {
  const saved = [binding(), binding('other.example')]
  const replacement: CloudflareDnsBinding = {
    domain: 'node.example',
    target: { kind: 'fixed_ip', ip: '1.1.1.1', runId: 'measured-run' },
  }
  expect(upsertCloudflareBinding(saved, replacement)).toEqual([
    saved[1],
    replacement,
  ])
  expect(saved).toHaveLength(2)
  const bindings = [binding()]
  vi.mocked(setCloudflareDnsBindings)
    .mockResolvedValueOnce(confirmation)
    .mockResolvedValueOnce({ status: 'applied', saved: true })
  await applyCloudflareDnsBindings(
    { profileUid: 'profile', fingerprint: 'fingerprint' },
    bindings.filter((item) => item.domain !== 'node.example'),
  )
  expect(setCloudflareDnsBindings).toHaveBeenNthCalledWith(
    1,
    'profile',
    'fingerprint',
    [],
    false,
  )
  expect(setCloudflareDnsBindings).toHaveBeenNthCalledWith(
    2,
    'profile',
    'fingerprint',
    [],
    false,
    'captured-token',
  )
  expect(bindings).toEqual([binding()])
})

it('retains stale and save failure outcomes without replaying or mutating the saved list', async () => {
  const context = { profileUid: 'profile', fingerprint: 'fingerprint' }
  const bindings = [binding()]
  const stale = { status: 'stale' as const, reason: 'Source changed' }
  vi.mocked(setCloudflareDnsBindings).mockResolvedValueOnce(stale)
  expect(await applyCloudflareDnsBindings(context, bindings)).toEqual(stale)
  expect(setCloudflareDnsBindings).toHaveBeenCalledOnce()
  const unsaved = {
    status: 'applied' as const,
    saved: false,
    error: 'Disk full',
  }
  vi.mocked(setCloudflareDnsBindings)
    .mockResolvedValueOnce(confirmation)
    .mockResolvedValueOnce(unsaved)
  vi.mocked(revalidateQueries).mockRejectedValueOnce(new Error('Read failed'))
  expect(await applyCloudflareDnsBindings(context, bindings)).toEqual(unsaved)
  expect(bindings).toEqual([binding()])
  expect(revalidateQueries).toHaveBeenCalledTimes(2)
})

it('stops after a repeated confirmation and refreshes after a rejected command', async () => {
  const context = { profileUid: 'profile', fingerprint: 'fingerprint' }
  vi.mocked(setCloudflareDnsBindings).mockResolvedValue(confirmation)
  expect(await applyCloudflareDnsBindings(context, [binding()])).toEqual({
    status: 'confirmation_failed',
  })
  expect(setCloudflareDnsBindings).toHaveBeenCalledTimes(2)
  vi.mocked(setCloudflareDnsBindings).mockRejectedValueOnce(
    new Error('Core rejected the binding'),
  )
  await expect(applyCloudflareDnsBindings(context, [])).rejects.toThrow(
    'Core rejected the binding',
  )
  expect(setCloudflareDnsBindings).toHaveBeenCalledTimes(3)
  expect(revalidateQueries).toHaveBeenCalledTimes(2)
})

it('offers only bindable proxy-server domains without dropping historical saved rows', () => {
  const node = {
    domain: 'node.example',
    bindable: true,
    occurrences: [
      {
        id: 'node',
        display: 'node.example',
        fieldPath: 'proxies[0].server',
        role: 'proxy_server' as const,
        bindable: true,
      },
    ],
  }
  const domains: CloudflareDnsInventory_Serialize['domains'] = [
    node,
    {
      ...node,
      domain: 'url.example',
      occurrences: [{ ...node.occurrences[0], role: 'url' }],
    },
    { ...node, domain: 'remote.example', bindable: false },
    { ...node, domain: 'unknown.example', occurrences: [] },
  ]
  const saved = [binding('url.example')]
  expect(eligibleCloudflareDomains(domains)).toEqual([node])
  expect(saved).toEqual([binding('url.example')])
})

it('rejects old runs, old revisions, and stale unavailable events, retaining same-run ordering', () => {
  const latest = snapshot('new', 8, '2026-10-19T12:00:00.123456789Z')
  const loaded = appStoreReducer(initialAppStoreState, {
    type: 'cloudflare/snapshot',
    snapshot: latest,
  })
  for (const incoming of [
    snapshot('new', 7, latest.startedAt),
    snapshot('old', 100, '2026-10-19T12:00:00.123456788Z'),
    snapshot('old', 100, '2026-10-19T12:00:00Z'),
  ]) {
    expect(
      appStoreReducer(loaded, {
        type: 'cloudflare/snapshot',
        snapshot: incoming,
      }),
    ).toBe(loaded)
  }
  expect(
    appStoreReducer(loaded, {
      type: 'cloudflare/unavailable',
      unavailable: { runId: 'old', revision: 100, reason: 'old failure' },
    }),
  ).toBe(loaded)
  expect(
    appStoreReducer(loaded, {
      type: 'cloudflare/unavailable',
      unavailable: { runId: 'new', revision: 7, reason: 'old failure' },
    }),
  ).toBe(loaded)
  const unavailable = appStoreReducer(loaded, {
    type: 'cloudflare/unavailable',
    unavailable: { runId: 'new', revision: 8, reason: 'Disconnected' },
  })
  expect(unavailable.cloudflareError).toBe('Disconnected')
  expect(
    appStoreReducer(unavailable, {
      type: 'cloudflare/snapshot',
      snapshot: { ...latest, revision: 9 },
    }).cloudflareError,
  ).toBeNull()
})
