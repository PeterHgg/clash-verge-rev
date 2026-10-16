import type { CandidateStatus } from '@/services/bindings'

export const candidateActivity = (
  candidate?: CandidateStatus,
): 'score' | 'keepObserving' | 'observing' | undefined => {
  if (
    !candidate ||
    ['unavailable', 'cooldown', 'untested'].includes(candidate.state)
  ) {
    return undefined
  }
  if (candidate.score != null && Number.isFinite(candidate.score)) {
    return 'score'
  }
  if (!candidate.reachable) return undefined
  return [candidate.meanRate, candidate.peakRate].some(
    (rate) => rate != null && Number.isFinite(rate) && rate > 0,
  )
    ? 'keepObserving'
    : 'observing'
}
