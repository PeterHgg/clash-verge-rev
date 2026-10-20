import { useCallback, useEffect } from 'react'

import { applyCloudflareDnsBindings } from '@/services/cloudflare-speed-test'
import {
  cancelCloudflareSpeedTest,
  getCloudflareDnsInventory,
  startCloudflareSpeedTest,
} from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { useQuery } from '@/services/query-client'
import {
  useAppDispatch,
  useAppReads,
  useCloudflareState,
} from '@/store/app-store-context'

export type CloudflareInventory = Awaited<
  ReturnType<typeof getCloudflareDnsInventory>
>
export type CloudflareBinding = CloudflareInventory['bindings'][number]
export type { CloudflareBindingOutcome } from '@/services/cloudflare-speed-test'

export const useCloudflareSpeedTest = (open: boolean) => {
  const state = useCloudflareState()
  const dispatch = useAppDispatch()
  const { readCloudflareSpeedTest } = useAppReads()
  const inventory = useQuery({
    queryKey: ['getCloudflareDnsInventory'],
    queryFn: getCloudflareDnsInventory,
    enabled: open,
    refetchOnWindowFocus: true,
    refetchOnReconnect: true,
    retry: false,
  })

  useEffect(() => {
    if (open) void readCloudflareSpeedTest()
  }, [open, readCloudflareSpeedTest, inventory.data?.restoreStatus])

  const start = useCallback(
    async (url: string, interfaceName: string) => {
      const result = await mutate(
        () => startCloudflareSpeedTest(url, interfaceName || undefined),
        {
          id: 'cloudflare-speed-test:start',
          errorNotice: false,
          onFulfilled: (snapshot) =>
            dispatch({ type: 'cloudflare/snapshot', snapshot }),
        },
      )
      return result.value
    },
    [dispatch],
  )

  const cancel = useCallback(
    async (runId: string) => {
      const result = await mutate(() => cancelCloudflareSpeedTest(runId), {
        id: `cloudflare-speed-test:cancel:${runId}`,
        errorNotice: false,
        onFulfilled: (snapshot) =>
          dispatch({ type: 'cloudflare/snapshot', snapshot }),
      })
      return result.value
    },
    [dispatch],
  )

  return {
    ...state,
    inventory,
    refresh: readCloudflareSpeedTest,
    start,
    cancel,
    apply: applyCloudflareDnsBindings,
  }
}
