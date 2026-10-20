import type { TranslationKey } from '@/types/generated/i18n-keys'

import type {
  CloudflareCapability,
  CloudflareDiagnostic,
  CloudflareDnsBinding,
  CloudflareDnsInventory,
  CloudflareDnsOutcome,
  CloudflareSnapshot,
} from './bindings'
import { setCloudflareDnsBindings } from './cmds'
import { mutate } from './mutate'
import { revalidateQueries } from './query-client'

export type CloudflareBindingOutcome =
  | CloudflareDnsOutcome
  | { status: 'confirmation_failed' }

export const applyCloudflareDnsBindings = async (
  context: Pick<CloudflareDnsInventory, 'profileUid' | 'fingerprint'>,
  bindings: CloudflareDnsBinding[],
): Promise<CloudflareBindingOutcome> => {
  const { profileUid, fingerprint } = context
  const captured = bindings.map((binding) => ({
    domain: binding.domain,
    target: { ...binding.target },
  }))
  const enabled = captured.length > 0
  const result = await mutate(
    async (): Promise<CloudflareBindingOutcome> => {
      try {
        const preview = await setCloudflareDnsBindings(
          profileUid,
          fingerprint,
          captured,
          enabled,
        )
        if (preview.status !== 'confirmation_required') return preview
        const confirmed = await setCloudflareDnsBindings(
          profileUid,
          fingerprint,
          captured,
          enabled,
          preview.confirmation,
        )
        return confirmed.status === 'confirmation_required'
          ? { status: 'confirmation_failed' }
          : confirmed
      } finally {
        // Query errors stay in their caches rather than replacing the write outcome.
        await revalidateQueries([
          ['getCloudflareDnsInventory'],
          ['getRuntimeConfig'],
        ]).catch(() => undefined)
      }
    },
    { id: 'cloudflare-dns-bindings', errorNotice: false },
  )
  return result.value
}

export const eligibleCloudflareDomains = (
  domains: CloudflareDnsInventory['domains'],
) =>
  domains.filter(
    (item) =>
      item.bindable &&
      item.occurrences.some(
        (entry) => entry.role === 'proxy_server' && entry.bindable,
      ),
  )

export const upsertCloudflareBinding = (
  bindings: CloudflareDnsBinding[],
  binding: CloudflareDnsBinding,
) => [...bindings.filter((item) => item.domain !== binding.domain), binding]

export const CLOUDFLARE_PREFERRED_DOMAINS = [
  'best.cloudflare.182682.xyz',
  'saas.sin.fan',
  'best.cloudflare.19931110.xyz',
] as const

export const CLOUDFLARE_TEST_URL =
  'https://speed.cloudflare.com/__down?bytes=99999999'

export type CloudflareResult = CloudflareSnapshot['results'][number]

export const supportsCloudflareDomainTest = (
  capability: CloudflareCapability | null,
) =>
  !!capability?.supported &&
  capability.domainOnly === true &&
  capability.engineVersion === 'cf-speedtest3' &&
  capability.method === 'verge-cfst-transfer-v3' &&
  capability.latencyMethod === 'tcp-connect'

const key = 'settings.modals.cloudflare'

export const cloudflareBindingRows = (
  inventory: Pick<CloudflareDnsInventory, 'bindings' | 'dormantDomains'>,
) =>
  inventory.bindings.map((binding) => ({
    binding,
    dormant: inventory.dormantDomains.includes(binding.domain),
  }))

export const formatCloudflareBindingStatus = (
  t: (key: TranslationKey) => string,
  inventory: CloudflareDnsInventory | undefined,
  outcome: CloudflareBindingOutcome | null,
): { severity: 'error' | 'waiting'; message: string } | null => {
  if (outcome?.status === 'stale') {
    return {
      severity: 'error',
      message: `${t(`${key}.stale`)} ${outcome.reason}`,
    }
  }
  if (outcome?.status === 'confirmation_failed') {
    return { severity: 'error', message: t(`${key}.confirmationFailed`) }
  }
  if (outcome?.status === 'applied' && (!outcome.saved || outcome.error)) {
    return {
      severity: 'error',
      message: [
        t(outcome.saved ? `${key}.notApplied` : `${key}.notSaved`),
        outcome.error,
      ]
        .filter(Boolean)
        .join(' '),
    }
  }
  if (!inventory?.enabled || !inventory.bindings.length) return null
  if (inventory.restoreStatus === 'pending') {
    return { severity: 'waiting', message: t(`${key}.bindingWaiting`) }
  }
  if (inventory.restoreStatus === 'restoring') {
    return { severity: 'waiting', message: t(`${key}.bindingRestoring`) }
  }
  if (inventory.restoreStatus === 'error') {
    return {
      severity: 'error',
      message: [t(`${key}.notApplied`), inventory.restoreReason]
        .filter(Boolean)
        .join(' '),
    }
  }
  return null
}

const diagnosticReasons = {
  resolution_failed: `${key}.diagnostics.reasons.resolutionFailed`,
  no_eligible_addresses: `${key}.diagnostics.reasons.noEligibleAddresses`,
  interface_changed: `${key}.diagnostics.reasons.interfaceChanged`,
  tcp_failed: `${key}.diagnostics.reasons.tcpFailed`,
  tls_failed: `${key}.diagnostics.reasons.tlsFailed`,
  setup_timeout: `${key}.diagnostics.reasons.setupTimeout`,
  request_failed: `${key}.diagnostics.reasons.requestFailed`,
  http_status: `${key}.diagnostics.reasons.httpStatus`,
  rate_limited: `${key}.diagnostics.reasons.rateLimited`,
  unsupported_encoding: `${key}.diagnostics.reasons.unsupportedEncoding`,
  unsupported_content: `${key}.diagnostics.reasons.unsupportedContent`,
  body_read_failed: `${key}.diagnostics.reasons.bodyReadFailed`,
  no_measured_data: `${key}.diagnostics.reasons.noMeasuredData`,
  cancelled: `${key}.diagnostics.reasons.cancelled`,
  deadline_exceeded: `${key}.diagnostics.reasons.deadlineExceeded`,
  no_candidates: `${key}.diagnostics.reasons.noCandidates`,
  no_completed_results: `${key}.diagnostics.reasons.noCompletedResults`,
} as const satisfies Record<CloudflareDiagnostic['code'], TranslationKey>

export const formatCloudflareDiagnostic = (
  t: (key: TranslationKey, options?: Record<string, unknown>) => string,
  diagnostic: CloudflareDiagnostic | null | undefined,
  legacyError?: string | null,
): string | null => {
  if (!diagnostic || !Object.hasOwn(diagnosticReasons, diagnostic.code)) {
    return legacyError || null
  }
  let reason: TranslationKey = diagnosticReasons[diagnostic.code]
  if (diagnostic.code === 'http_status') {
    if (diagnostic.httpStatus === 403) {
      reason = `${key}.diagnostics.reasons.forbidden`
    } else if (diagnostic.httpStatus != null && diagnostic.httpStatus >= 500) {
      reason = `${key}.diagnostics.reasons.serverError`
    }
  }
  const parts = [t(`${key}.diagnostics.stages.${diagnostic.stage}`), t(reason)]
  if (diagnostic.httpStatus != null) {
    parts.push(
      t(`${key}.diagnostics.httpStatus`, { status: diagnostic.httpStatus }),
    )
  }
  if (diagnostic.contentType != null) {
    parts.push(
      t(`${key}.diagnostics.contentType`, {
        type: t(`${key}.diagnostics.contentTypes.${diagnostic.contentType}`),
      }),
    )
  }
  if (diagnostic.retryAfterSeconds != null) {
    parts.push(
      t(`${key}.diagnostics.retryAfter`, {
        seconds: diagnostic.retryAfterSeconds,
      }),
    )
  }
  return parts.join(' · ')
}

export const summarizeCloudflareSamples = (
  results: readonly CloudflareResult[],
  domain: string,
) => {
  const samples = results.filter((result) => result.sources.includes(domain))
  const success = samples.filter(hasCloudflareDownload).length
  const failed = samples.filter((result) => result.state === 'failed').length
  return {
    samples,
    success,
    failed,
    untested: samples.length - success - failed,
  }
}

const timestampOrder = (timestamp: string) =>
  timestamp.replace(
    /(?:\.(\d+))?Z$/,
    (_, fraction: string | undefined) => `.${(fraction ?? '').padEnd(9, '0')}Z`,
  )

export const isNewerCloudflareSnapshot = (
  current: CloudflareSnapshot | null,
  incoming: CloudflareSnapshot,
) => {
  if (!current) return true
  if (current.runId === incoming.runId) {
    return incoming.revision > current.revision
  }
  return timestampOrder(incoming.startedAt) > timestampOrder(current.startedAt)
}

export const hasCloudflareDownload = (result: CloudflareResult) =>
  (result.state === 'complete' || result.state === 'short') &&
  Number.isInteger(result.successfulRounds) &&
  Number.isInteger(result.attemptedRounds) &&
  result.successfulRounds >= (result.state === 'complete' ? 2 : 1) &&
  result.successfulRounds <= result.attemptedRounds &&
  result.attemptedRounds <= 3 &&
  result.meanRate != null &&
  Number.isFinite(result.meanRate) &&
  result.meanRate > 0 &&
  result.bytesRead > 0 &&
  Number.isFinite(result.durationMs) &&
  result.durationMs > 0

const cloudflareScore = (result: CloudflareResult) =>
  result.score != null &&
  Number.isFinite(result.score) &&
  result.score >= 0 &&
  result.score <= 1
    ? result.score
    : null

export const rankCloudflareResults = (
  results: readonly CloudflareResult[],
): CloudflareResult[] =>
  [...results].sort((a, b) => {
    const measuredA = hasCloudflareDownload(a)
    const measuredB = hasCloudflareDownload(b)
    if (measuredA !== measuredB) return measuredA ? -1 : 1
    if (measuredA && measuredB) {
      const score = (cloudflareScore(b) ?? -1) - (cloudflareScore(a) ?? -1)
      if (score) return score
      const complete =
        Number(b.state === 'complete') - Number(a.state === 'complete')
      if (complete) return complete
    }
    return a.ip.localeCompare(b.ip)
  })

export const formatCloudflareScore = (result: CloudflareResult) => {
  const score = cloudflareScore(result)
  return hasCloudflareDownload(result) && score != null
    ? (score * 100).toFixed(1)
    : '—'
}

export const formatCloudflareRounds = (
  t: (key: TranslationKey, options?: Record<string, unknown>) => string,
  result: Pick<CloudflareResult, 'successfulRounds' | 'attemptedRounds'>,
) =>
  t(`${key}.roundCounts`, {
    successfulRounds: result.successfulRounds,
    attemptedRounds: result.attemptedRounds,
  })

export const formatCloudflareBytes = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`
}

export const formatCloudflareMbps = (rate: number | null | undefined) =>
  rate == null ? '—' : ((rate * 8) / 1_000_000).toFixed(2)
