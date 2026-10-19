import {
  Alert,
  Box,
  Button,
  MenuItem,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  TextField,
  Typography,
} from '@mui/material'
import { forwardRef, useImperativeHandle, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { BaseDialog, type DialogRef } from '@/components/base'
import {
  type CloudflareBinding,
  type CloudflareBindingOutcome,
  useCloudflareSpeedTest,
} from '@/hooks/use-cloudflare-speed-test'
import {
  CLOUDFLARE_PREFERRED_DOMAINS,
  CLOUDFLARE_TEST_URL,
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
} from '@/services/cloudflare-speed-test'
import { errorDetail } from '@/services/notice-service'

const key = 'settings.modals.cloudflare'
const sourceStates = [
  'pending',
  'resolving',
  'resolved',
  'ready',
  'failed',
  'complete',
  'completed',
  'untested',
] as const

export const CloudflareSpeedTestViewer = forwardRef<DialogRef>((_, ref) => {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [url, setUrl] = useState(CLOUDFLARE_TEST_URL)
  const [interfaceName, setInterfaceName] = useState('')
  const [domain, setDomain] = useState('')
  const [target, setTarget] = useState('')
  const [busy, setBusy] = useState<
    'start' | 'cancel' | 'apply' | 'refresh' | null
  >(null)
  const [error, setError] = useState<string | null>(null)
  const [outcome, setOutcome] = useState<CloudflareBindingOutcome | null>(null)
  const api = useCloudflareSpeedTest(open)
  const inventory = api.inventory.data
  const snapshot = api.snapshot
  const capability = api.capability
  const supported = supportsCloudflareDomainTest(capability)
  const active = snapshot?.state === 'running'
  const snapshotReason =
    snapshot?.diagnostic?.code === 'cancelled'
      ? null
      : formatCloudflareDiagnostic(t, snapshot?.diagnostic, snapshot?.error)
  const results = rankCloudflareResults(snapshot?.results ?? [])
  const measured = results.filter(hasCloudflareDownload)
  const domains = eligibleCloudflareDomains(inventory?.domains ?? [])
  const selectedDomain = domains.some((item) => item.domain === domain)
    ? domain
    : ''
  const selected = measured.find((item) => `ip:${item.ip}` === target)
  const preferred = CLOUDFLARE_PREFERRED_DOMAINS.find(
    (item) => `domain:${item}` === target,
  )
  const validTarget =
    (preferred && capability?.dnsAliases) ||
    (selected && snapshot && capability?.fixedAddressFamily)

  const close = () => setOpen(false)

  useImperativeHandle(ref, () => ({
    open: () => {
      setOpen(true)
      setError(null)
      setOutcome(null)
    },
    close,
  }))

  const run = async (intent: 'start' | 'cancel') => {
    setBusy(intent)
    setError(null)
    try {
      if (intent === 'start') await api.start(url, interfaceName)
      else if (snapshot) await api.cancel(snapshot.runId)
    } catch (failure) {
      setError(errorDetail(failure))
    } finally {
      setBusy(null)
    }
  }

  const refresh = async () => {
    setBusy('refresh')
    setError(null)
    try {
      await Promise.all([api.refresh(), api.inventory.refetch()])
    } catch (failure) {
      setError(errorDetail(failure))
    } finally {
      setBusy(null)
    }
  }

  const persist = async (bindings: CloudflareBinding[]) => {
    if (!inventory) return
    setBusy('apply')
    setError(null)
    setOutcome(null)
    try {
      setOutcome(await api.apply(inventory, bindings))
    } catch (failure) {
      setError(errorDetail(failure))
    } finally {
      setBusy(null)
    }
  }

  const apply = () => {
    if (!inventory || !selectedDomain || !validTarget) return
    const binding: CloudflareBinding = preferred
      ? {
          domain: selectedDomain,
          target: { kind: 'preferred_domain', domain: preferred },
        }
      : {
          domain: selectedDomain,
          target: {
            kind: 'fixed_ip',
            ip: selected!.ip,
            runId: snapshot!.runId,
          },
        }
    void persist(upsertCloudflareBinding(inventory.bindings, binding))
  }

  return (
    <BaseDialog
      open={open}
      title={t(`${key}.title`)}
      maxWidth="lg"
      onClose={close}
      onCancel={close}
      cancelBtn={t('shared.actions.close')}
      disableOk
      contentSx={{
        width: 'min(1000px, calc(100vw - 64px))',
        maxWidth: '100%',
        minWidth: 0,
        boxSizing: 'border-box',
        overflowX: 'hidden',
        '& .MuiTypography-root': { overflowWrap: 'anywhere' },
      }}
    >
      <Stack spacing={2}>
        <Box>
          <Typography variant="h6" sx={{ mb: 1 }}>
            {t(`${key}.testTitle`)}
          </Typography>
          <Stack spacing={1}>
            <Stack direction={{ xs: 'column', md: 'row' }} spacing={1}>
              <TextField
                label={t(`${key}.testUrl`)}
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                disabled={active || busy === 'start'}
                fullWidth
                sx={{ minWidth: 0 }}
                size="small"
              />
              <TextField
                label={t(`${key}.interface`)}
                value={interfaceName}
                onChange={(event) => setInterfaceName(event.target.value)}
                disabled={active || busy === 'start'}
                size="small"
                sx={{
                  minWidth: 0,
                  width: { xs: '100%', md: 230 },
                  flexShrink: 0,
                }}
              />
            </Stack>
            <Stack
              direction="row"
              spacing={1}
              useFlexGap
              sx={{ flexWrap: 'wrap' }}
            >
              <Button
                variant="contained"
                disabled={!supported || !!api.error || active || !!busy || !url}
                onClick={() => void run('start')}
              >
                {t(`${key}.start`)}
              </Button>
              <Button
                disabled={!active || !!busy}
                onClick={() => void run('cancel')}
              >
                {t('shared.actions.cancel')}
              </Button>
              <Button disabled={!!busy} onClick={() => void refresh()}>
                {t('shared.actions.refresh')}
              </Button>
            </Stack>
            {!capability && !api.error && (
              <Typography variant="body2">
                {t('shared.statuses.loading')}
              </Typography>
            )}
            {capability && !supported && !api.error && (
              <Alert severity="error">
                {t(`${key}.capabilityNotice`)} {capability.reason}
              </Alert>
            )}
            {(error || api.error) && (
              <Alert severity="error">{error || api.error}</Alert>
            )}
            {snapshot && (
              <Box aria-live="polite">
                <Typography variant="body2">
                  {t(`${key}.states.${snapshot.state}`)} ·{' '}
                  {t(`${key}.stages.${snapshot.stage}`)} ·{' '}
                  {snapshot.completedTargets}/{snapshot.totalTargets} ·{' '}
                  {t(`${key}.downloaded`)}{' '}
                  {formatCloudflareBytes(snapshot.bytesRead)} /{' '}
                  {formatCloudflareBytes(snapshot.budgetBytes)}
                </Typography>
                {snapshotReason && (
                  <Typography variant="body2" color="error">
                    {snapshotReason}
                  </Typography>
                )}
              </Box>
            )}
          </Stack>
        </Box>

        <Box>
          <Typography variant="h6" sx={{ mb: 1 }}>
            {t(`${key}.bindingsTitle`)}
          </Typography>
          <Stack spacing={1}>
            {api.inventory.error && (
              <Alert severity="error">{errorDetail(api.inventory.error)}</Alert>
            )}
            {api.inventory.isPending && (
              <Typography variant="body2">
                {t('shared.statuses.loading')}
              </Typography>
            )}
            <Stack direction={{ xs: 'column', md: 'row' }} spacing={1}>
              <TextField
                select
                size="small"
                label={t(`${key}.sourceDomain`)}
                value={selectedDomain}
                onChange={(event) => setDomain(event.target.value)}
                fullWidth
                sx={{ minWidth: 0 }}
                disabled={!inventory || !!busy}
              >
                {domains.map((item) => (
                  <MenuItem key={item.domain} value={item.domain}>
                    {item.domain}
                  </MenuItem>
                ))}
              </TextField>
              <TextField
                select
                size="small"
                label={t(`${key}.target`)}
                value={preferred || selected ? target : ''}
                onChange={(event) => setTarget(event.target.value)}
                fullWidth
                sx={{ minWidth: 0 }}
                disabled={!inventory || !!busy}
              >
                {CLOUDFLARE_PREFERRED_DOMAINS.map((name) => (
                  <MenuItem
                    key={name}
                    value={`domain:${name}`}
                    disabled={!capability?.dnsAliases}
                  >
                    {name} · {t(`${key}.dynamic`)}
                  </MenuItem>
                ))}
                {measured.map((item) => (
                  <MenuItem
                    key={item.ip}
                    value={`ip:${item.ip}`}
                    disabled={!capability?.fixedAddressFamily}
                  >
                    {item.ip} · {t(`${key}.fixed`)}
                  </MenuItem>
                ))}
              </TextField>
              <Button
                variant="contained"
                sx={{
                  flexShrink: 0,
                  alignSelf: { xs: 'stretch', md: 'center' },
                }}
                disabled={
                  !inventory ||
                  !!api.inventory.error ||
                  !selectedDomain ||
                  !validTarget ||
                  !!busy
                }
                onClick={apply}
              >
                {t(`${key}.apply`)}
              </Button>
            </Stack>
            {inventory?.bindings.map((binding) => (
              <Stack
                key={binding.domain}
                direction="row"
                spacing={1}
                sx={{
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  minWidth: 0,
                }}
              >
                <Typography variant="body2" sx={{ minWidth: 0 }}>
                  {binding.domain} →{' '}
                  {binding.target.kind === 'fixed_ip'
                    ? binding.target.ip
                    : binding.target.domain}
                </Typography>
                <Button
                  sx={{ flexShrink: 0 }}
                  disabled={!!busy}
                  onClick={() =>
                    void persist(
                      inventory.bindings.filter(
                        (item) => item.domain !== binding.domain,
                      ),
                    )
                  }
                >
                  {t('shared.actions.delete')}
                </Button>
              </Stack>
            ))}
            {inventory && !inventory.bindings.length && (
              <Typography variant="body2" color="text.secondary">
                {t('shared.statuses.empty')}
              </Typography>
            )}
            {inventory?.enabled &&
              !inventory.effectiveEnabled &&
              (!outcome ||
                (outcome.status === 'applied' &&
                  outcome.saved &&
                  !outcome.error)) && (
                <Alert severity="error">{t(`${key}.notApplied`)}</Alert>
              )}
            {outcome?.status === 'stale' && (
              <Alert severity="error">
                {t(`${key}.stale`)} {outcome.reason}
              </Alert>
            )}
            {outcome?.status === 'confirmation_failed' && (
              <Alert severity="error">{t(`${key}.confirmationFailed`)}</Alert>
            )}
            {outcome?.status === 'applied' &&
              (!outcome.saved || outcome.error) && (
                <Alert severity="error">
                  {t(outcome.saved ? `${key}.notApplied` : `${key}.notSaved`)}{' '}
                  {outcome.error}
                </Alert>
              )}
          </Stack>
        </Box>

        <Box>
          <Typography variant="h6" sx={{ mb: 1 }}>
            {t(`${key}.resultsTitle`)}
          </Typography>
          <Stack spacing={0.5} sx={{ mb: 1 }}>
            {CLOUDFLARE_PREFERRED_DOMAINS.map((name) => {
              const source = snapshot?.sources.find(
                (item) => item.name === name,
              )
              const state = sourceStates.find(
                (state) => state === source?.state,
              )
              const { success, failed, untested } = summarizeCloudflareSamples(
                results,
                name,
              )
              const reason = formatCloudflareDiagnostic(
                t,
                source?.diagnostic,
                source?.error,
              )
              return (
                <Box key={name}>
                  <Typography variant="body2">
                    {name} ·{' '}
                    {state
                      ? t(`${key}.sourceStates.${state}`)
                      : t(`${key}.notTested`)}{' '}
                    · {t(`${key}.sampleCounts`, { success, failed, untested })}
                  </Typography>
                  {reason && (
                    <Typography variant="body2" color="error">
                      {reason}
                    </Typography>
                  )}
                </Box>
              )
            })}
          </Stack>
          <TableContainer
            sx={{
              maxHeight: 330,
              width: '100%',
              minWidth: 0,
              overflowX: 'auto',
              boxSizing: 'border-box',
              border: 1,
              borderColor: 'divider',
              borderRadius: 1,
            }}
          >
            <Table
              size="small"
              stickyHeader
              sx={{ minWidth: 760 }}
              aria-label={t(`${key}.resultsTitle`)}
            >
              <TableHead>
                <TableRow>
                  {(
                    [
                      'ipSource',
                      'score',
                      'meanMbps',
                      'latency',
                      'stability',
                      'state',
                      'sample',
                    ] as const
                  ).map((field) => (
                    <TableCell key={field}>{t(`${key}.${field}`)}</TableCell>
                  ))}
                </TableRow>
              </TableHead>
              <TableBody>
                {results.map((result) => {
                  const reason =
                    result.diagnostic?.code === 'cancelled'
                      ? null
                      : formatCloudflareDiagnostic(
                          t,
                          result.diagnostic,
                          result.error,
                        )
                  const selectable =
                    hasCloudflareDownload(result) &&
                    capability?.fixedAddressFamily
                  return (
                    <TableRow
                      key={result.ip}
                      hover={!!selectable}
                      selected={target === `ip:${result.ip}`}
                    >
                      <TableCell>
                        <Button
                          size="small"
                          disabled={!selectable || !!busy}
                          onClick={() => setTarget(`ip:${result.ip}`)}
                          sx={{ p: 0, minWidth: 0, textTransform: 'none' }}
                        >
                          {result.ip}
                        </Button>
                        <Typography variant="caption" component="div">
                          {result.sources.join(', ')}
                        </Typography>
                      </TableCell>
                      <TableCell>{formatCloudflareScore(result)}</TableCell>
                      <TableCell>
                        {formatCloudflareMbps(result.meanRate)}
                      </TableCell>
                      <TableCell>
                        {snapshot?.latencyMethod !== 'tcp-connect' ||
                        result.latencyMs == null
                          ? '—'
                          : result.latencyMs.toFixed(1)}
                      </TableCell>
                      <TableCell>
                        {result.stability == null
                          ? '—'
                          : `${(result.stability * 100).toFixed(0)}%`}
                      </TableCell>
                      <TableCell>
                        {t(`${key}.resultStates.${result.state}`)}
                        <Typography variant="caption" component="div">
                          {formatCloudflareRounds(t, result)}
                        </Typography>
                        {reason && (
                          <Typography
                            variant="caption"
                            color="error"
                            component="div"
                          >
                            {reason}
                          </Typography>
                        )}
                      </TableCell>
                      <TableCell>
                        <Box component="details">
                          <Box component="summary" sx={{ cursor: 'pointer' }}>
                            {formatCloudflareBytes(result.bytesRead)} ·{' '}
                            {(result.durationMs / 1000).toFixed(2)} s
                          </Box>
                          <Typography variant="caption" component="div">
                            {t(`${key}.p90Mbps`)}:{' '}
                            {formatCloudflareMbps(result.peakRate)}
                          </Typography>
                          <Typography variant="caption" component="div">
                            {t(`${key}.failures`)}:{' '}
                            {result.attempts
                              ? `${((100 * result.failures) / result.attempts).toFixed(0)}% (${result.failures}/${result.attempts})`
                              : '—'}
                          </Typography>
                          {result.finishReason && (
                            <Typography variant="caption" component="div">
                              {t(`${key}.finishReasons.${result.finishReason}`)}
                            </Typography>
                          )}
                        </Box>
                      </TableCell>
                    </TableRow>
                  )
                })}
                {!results.length && (
                  <TableRow>
                    <TableCell colSpan={7}>{t(`${key}.notTested`)}</TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          </TableContainer>
        </Box>
      </Stack>
    </BaseDialog>
  )
})
