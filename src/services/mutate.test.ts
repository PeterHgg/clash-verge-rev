import { expect, it, vi } from 'vitest'

import { mutate } from './mutate'

it('keeps manual Cloudflare cancellation independent of a pending start intent', async () => {
  let resolveStart!: () => void
  const start = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        resolveStart = resolve
      }),
  )
  expect(start).not.toHaveBeenCalled()
  const pending = mutate(start, { id: 'cloudflare-speed-test:start' })
  const cancel = vi.fn().mockResolvedValue(undefined)
  await mutate(cancel, { id: 'cloudflare-speed-test:cancel:run' })
  expect(cancel).toHaveBeenCalledOnce()
  resolveStart()
  await pending
})

vi.mock('@/services/notice-service', () => ({
  showNotice: { error: vi.fn(), warning: vi.fn(), success: vi.fn() },
}))
vi.mock('@/services/query-client', () => ({ revalidateQueries: vi.fn() }))

it('executes every queued setting write even when its predecessor fails', async () => {
  let reject!: (error: Error) => void
  const first = mutate(
    () =>
      new Promise<void>((_resolve, rejectFirst) => {
        reject = rejectFirst
      }),
    { id: 'patch-verge-config', errorNotice: false },
  ).catch((error) => error)
  const writeSecond = vi.fn().mockResolvedValue(undefined)
  const onFulfilled = vi.fn()
  const second = mutate(writeSecond, {
    id: 'patch-verge-config',
    onFulfilled,
    errorNotice: false,
  })
  expect(writeSecond).not.toHaveBeenCalled()
  const error = new Error('first write failed')
  reject(error)
  expect(await first).toBe(error)
  expect(await second).toEqual({ ok: true, value: undefined })
  expect(writeSecond).toHaveBeenCalledOnce()
  expect(onFulfilled).toHaveBeenCalledOnce()
})
