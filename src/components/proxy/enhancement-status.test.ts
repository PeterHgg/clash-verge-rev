import { describe, expect, it } from 'vitest'

import type { CandidateStatus } from '@/services/bindings'

import { candidateActivity } from './enhancement-status'

const candidate = (patch: Partial<CandidateStatus> = {}): CandidateStatus => ({
  name: 'node',
  score: null,
  selected: true,
  reachable: true,
  freshHealth: true,
  proven: false,
  confidence: 0,
  meanRate: 0,
  peakRate: 0,
  latencyMs: 50,
  state: 'selected',
  cooldownRemainingSeconds: null,
  lastCheckedSecondsAgo: null,
  ...patch,
})

describe('candidateActivity', () => {
  it('keeps unmeasured and measured-but-unproven observations distinct without guessing inactivity', () => {
    expect(candidateActivity(candidate())).toBe('observing')
    expect(candidateActivity(candidate({ meanRate: 1024 }))).toBe(
      'keepObserving',
    )
    expect(candidateActivity(candidate({ peakRate: 1024 }))).toBe(
      'keepObserving',
    )
    expect(candidateActivity(candidate({ meanRate: Number.NaN }))).toBe(
      'observing',
    )
  })

  it('shows a finite score, including zero, without changing the primary role', () => {
    for (const state of ['selected', 'trial', 'standby', 'probing']) {
      expect(
        candidateActivity(candidate({ state, score: 0, proven: true })),
      ).toBe('score')
    }
    expect(
      candidateActivity(candidate({ score: Number.POSITIVE_INFINITY })),
    ).toBe('observing')
  })

  it('does not add an observation qualifier to missing, unavailable, or waiting candidates', () => {
    expect(candidateActivity()).toBeUndefined()
    expect(candidateActivity(candidate({ reachable: false }))).toBeUndefined()
    for (const state of ['unavailable', 'cooldown', 'untested']) {
      expect(
        candidateActivity(candidate({ state, meanRate: 1024 })),
      ).toBeUndefined()
    }
  })
})
